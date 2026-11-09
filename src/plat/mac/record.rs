//! 录屏截屏：ScreenCaptureKit 的 SCStream 只截选区那块（sourceRect），按帧率把 BGRA 帧送到后台串行队列，交给回调。
//! 画面没变时 SCStream 送的帧不带图像（idle），直接跳过，GIF 那边让上一帧停得更久。
//! 录屏自己的边框和控制条在过滤器里排除掉；屏幕录制权限在框选前截图时已经确认过。

use super::screen;
use crate::plat::{Kind, OnFrame, Rect, Win};
use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_core_foundation::CGRect;
use objc2_core_graphics::kCGColorSpaceSRGB;
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{
    CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferUnlockBaseAddress, kCVPixelFormatType_32BGRA,
};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamOutput, SCStreamOutputType};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// 帧回调和开始录制的时间；停的时候摘掉，之后来的帧都丢掉
type Sink = Arc<Mutex<Option<(OnFrame, Instant)>>>;
type OnError = Arc<Mutex<Option<Box<dyn FnOnce(String) + Send>>>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct OutputIvars {
    sink: Sink,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[ivars = OutputIvars]
    struct FrameOutput;

    unsafe impl NSObjectProtocol for FrameOutput {}

    unsafe impl SCStreamOutput for FrameOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output(&self, _stream: &SCStream, buffer: &CMSampleBuffer, kind: SCStreamOutputType) {
            if kind != SCStreamOutputType::Screen {
                return;
            }
            let mut g = lock(&self.ivars().sink);
            let Some((f, t0)) = g.as_mut() else { return };
            // 画面没变的帧不带图像
            let Some(img) = (unsafe { buffer.image_buffer() }) else { return };
            unsafe {
                if CVPixelBufferLockBaseAddress(&img, CVPixelBufferLockFlags::ReadOnly) != 0 {
                    return;
                }
                let base = CVPixelBufferGetBaseAddress(&img) as *const u8;
                let (stride, h) = (CVPixelBufferGetBytesPerRow(&img), CVPixelBufferGetHeight(&img));
                if !base.is_null() {
                    f(std::slice::from_raw_parts(base, stride * h), stride, t0.elapsed());
                }
                CVPixelBufferUnlockBaseAddress(&img, CVPixelBufferLockFlags::ReadOnly);
            }
        }
    }
);

/// 开始录制以后才有的流：`stopped` 表示已经停了（流晚于停止才建好时直接不开始）
#[derive(Default)]
struct State {
    stream: Option<Retained<SCStream>>,
    output: Option<Retained<FrameOutput>>,
    stopped: bool,
}

/// 正在录：`stop` 或者丢掉它就停；返回后不会再调帧回调
pub struct Recording {
    sink: Sink,
    state: Arc<Mutex<State>>,
}

impl Recording {
    pub fn stop(self) {}
}

impl Drop for Recording {
    fn drop(&mut self) {
        // 先摘掉回调：正在处理的那一帧处理完才拿得到锁
        lock(&self.sink).take();
        let mut st = lock(&self.state);
        st.stopped = true;
        if let Some(s) = st.stream.take() {
            unsafe { s.stopCaptureWithCompletionHandler(None) };
        }
        st.output = None;
    }
}

fn message(err: *mut NSError) -> String {
    unsafe { err.as_ref() }.map(|e| e.localizedDescription().to_string()).unwrap_or_else(|| "无法录制屏幕".into())
}

/// 建流要的东西：拿到系统给的可截内容以后用
#[derive(Clone)]
struct Setup<R> {
    display_id: u32,
    /// 区域在显示器上的位置（点）和大小（像素）
    source: CGRect,
    w: i32,
    h: i32,
    fps: u32,
    cursor: bool,
    /// 录屏自己的边框和控制条的窗口编号
    skip: Vec<u32>,
    sink: Sink,
    state: Arc<Mutex<State>>,
    report: R,
}

/// 开始录区域 `r`（全局桌面物理像素），每秒最多 `fps` 帧，`cursor`：帧里带鼠标指针。流是异步建好的：建不成、开始失败时在别的线程上调 `on_error`
pub fn record(r: Rect, fps: u32, cursor: bool, on_frame: OnFrame, on_error: impl FnOnce(String) + Send + 'static) -> Result<Recording, String> {
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if w <= 0 || h <= 0 {
        return Err("录制区域是空的".into());
    }
    let (display_id, source) = screen::display_rect(r).ok_or("找不到这块显示器")?;
    // 录屏自己的边框和控制条：按系统给的窗口编号排除
    let skip: Vec<u32> = Win::all(Kind::RecFrame).into_iter().chain(Win::all(Kind::RecBar)).filter_map(|w| w.window_number()).collect();
    let sink: Sink = Arc::new(Mutex::new(Some((on_frame, Instant::now()))));
    let state = Arc::new(Mutex::new(State::default()));
    let errors: OnError = Arc::new(Mutex::new(Some(Box::new(on_error))));
    let report = move |e: String| {
        if let Some(f) = lock(&errors).take() {
            f(e);
        }
    };
    fetch(Setup { display_id, source, w, h, fps, cursor, skip, sink: sink.clone(), state: state.clone(), report }, 0);
    Ok(Recording { sink, state })
}

/// 向系统要可截的内容，排除掉录屏自己的窗口建流开始截。点了就录时边框、控制条刚出来，系统给的窗口列表里
/// 可能还没有它们（那样会被录进去）：缺了就过 50 ms 再要一次，最多 10 次，还缺就排除找到的那些照样开始
fn fetch<R: Fn(String) + Clone + Send + 'static>(s: Setup<R>, tries: u32) {
    let content = RcBlock::new(move |content: *mut SCShareableContent, err: *mut NSError| {
        let report = &s.report;
        let Some(content) = (unsafe { content.as_ref() }) else { return report(message(err)) };
        let displays = unsafe { content.displays() };
        let Some(display) = displays.iter().find(|d| unsafe { d.displayID() } == s.display_id) else { return report("显示器已断开".into()) };
        let windows: Vec<_> = unsafe { content.windows() }.iter().filter(|w| s.skip.contains(&unsafe { w.windowID() })).collect();
        if windows.len() < s.skip.len() && tries < 10 && !lock(&s.state).stopped {
            // 回调在后台队列上，等一下不碍界面
            std::thread::sleep(Duration::from_millis(50));
            return fetch(s.clone(), tries + 1);
        }
        let filter = unsafe { SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &display, &NSArray::from_retained_slice(&windows)) };
        let config = unsafe { SCStreamConfiguration::new() };
        unsafe {
            config.setSourceRect(s.source);
            config.setWidth(s.w as usize);
            config.setHeight(s.h as usize);
            config.setPixelFormat(kCVPixelFormatType_32BGRA);
            config.setShowsCursor(s.cursor);
            config.setMinimumFrameInterval(CMTime::new(1, s.fps.clamp(1, 60) as i32));
            config.setQueueDepth(3);
            config.setColorSpaceName(kCGColorSpaceSRGB);
        }
        let stream = unsafe { SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &config, None) };
        let output = FrameOutput::alloc().set_ivars(OutputIvars { sink: s.sink.clone() });
        let output: Retained<FrameOutput> = unsafe { msg_send![super(output), init] };
        let queue = DispatchQueue::new("jie.record", None);
        if let Err(e) = unsafe { stream.addStreamOutput_type_sampleHandlerQueue_error(ProtocolObject::from_ref(&*output), SCStreamOutputType::Screen, Some(&queue)) } {
            return report(e.localizedDescription().to_string());
        }
        let mut st = lock(&s.state);
        // 还没建好就已经停了
        if st.stopped {
            return;
        }
        let report = report.clone();
        let started = RcBlock::new(move |err: *mut NSError| {
            if !err.is_null() {
                report(message(err));
            }
        });
        unsafe { stream.startCaptureWithCompletionHandler(Some(&started)) };
        (st.stream, st.output) = (Some(stream), Some(output));
    });
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&content) };
}
