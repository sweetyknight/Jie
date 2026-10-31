//! 文字标注的输入：插入光标、多行、粘贴；输入法的组合串画在光标处，候选窗跟着光标走。

use super::{Ann, COLORS, Editor, FONT, PEN, Pt, Shape, grow, outer, text_layout};
use crate::plat::{self, Canvas, Event, Key, Rect, RectF, Stroke, Text};
use crate::ui::{self, rectf};

/// 正在输入的文字：文字框左上角、内容、插入光标（字节位置）；
/// 输入法还没上屏的组合串和它里面的光标（UTF-16 位置），只画出来、不进内容
#[derive(Default)]
pub struct Edit {
    pub at: Pt,
    pub text: String,
    caret: usize,
    comp: String,
    comp_cur: usize,
}

impl Edit {
    /// 在光标处插入：\r\n、\r 都当换行，其余控制字符不要
    fn insert(&mut self, s: &str) {
        let s: String = s.replace("\r\n", "\n").replace('\r', "\n").chars().filter(|c| *c == '\n' || !c.is_control()).collect();
        self.text.insert_str(self.caret, &s);
        self.caret += s.len();
    }

    /// 左右方向键、Home / End（到当前行的头尾）、Delete
    fn key(&mut self, k: Key) {
        let (t, c) = (&self.text, self.caret);
        match k {
            Key::Left => self.caret = t[..c].chars().next_back().map_or(c, |ch| c - ch.len_utf8()),
            Key::Right => self.caret = t[c..].chars().next().map_or(c, |ch| c + ch.len_utf8()),
            Key::Home => self.caret = t[..c].rfind('\n').map_or(0, |i| i + 1),
            Key::End => self.caret = t[c..].find('\n').map_or(t.len(), |i| c + i),
            Key::Delete if c < t.len() => _ = self.text.remove(c),
            _ => {}
        }
    }

    /// 输入法：上屏的字插在光标处，组合串换成新的
    pub fn compose(&mut self, result: Option<&str>, comp: String, cur: usize) {
        if let Some(r) = result {
            self.insert(r);
        }
        self.comp_cur = cur.min(comp.encode_utf16().count());
        self.comp = comp;
    }
}

/// 图像坐标的矩形换成客户区坐标
fn client(r: RectF, to_client: &impl Fn(Pt) -> Pt) -> RectF {
    let (a, b) = (to_client(Pt::new(r.left, r.top)), to_client(Pt::new(r.right, r.bottom)));
    rectf(a.x, a.y, b.x, b.y)
}

impl Editor {
    /// 文字工具点在 p：从这里开始输入，p 落在第一行的竖直中间
    pub(super) fn start_text(&mut self, p: Pt) {
        let fs = FONT[PEN.get().1] * self.scale;
        self.text = Some(Edit { at: Pt::new(p.x, p.y - fs * 0.65), ..Edit::default() });
    }

    /// 结束输入：去掉末尾空白，有内容就变成文字标注（提交后不能再改内容，要改就删掉重写）
    pub fn finish_text(&mut self) {
        let Some(e) = self.text.take() else { return };
        let t = e.text.trim_end();
        if !t.is_empty() {
            let (color, size) = PEN.get();
            self.doc.checkpoint();
            self.doc.anns.push(Ann { shape: Shape::Text(e.at, t.to_string()), color, size });
        }
    }

    /// 输入框里的排版（组合串插在光标处、带下划线）和插入光标（1 DIP 宽、一行高），图像像素
    fn edit_layout(&self) -> Option<(&Edit, Text, RectF)> {
        let e = self.text.as_ref()?;
        let (head, tail) = e.text.split_at(e.caret);
        let l = text_layout(&format!("{head}{}{tail}", e.comp), FONT[PEN.get().1] * self.scale)?;
        let at = head.encode_utf16().count() as u32;
        if !e.comp.is_empty() {
            l.underline(at, e.comp.encode_utf16().count() as u32);
        }
        let (x, top, h) = l.caret(at + e.comp_cur as u32);
        let x = (e.at.x + x).round();
        let top = (e.at.y + top).round();
        Some((e, l, rectf(x, top, x + self.scale.round().max(1.0), top + h.round())))
    }

    /// 输入框（虚线框）的范围：左 5、上 3 的内边距，右边多留一个字宽
    pub(super) fn text_bounds(&self) -> Option<RectF> {
        let (e, l, _) = self.edit_layout()?;
        let (s, fs) = (self.scale, FONT[PEN.get().1] * self.scale);
        let lines = e.text.split('\n').count() as f32;
        let (x, y) = (e.at.x - 5.0 * s, e.at.y - 3.0 * s);
        Some(rectf(x, y, x + l.width().max(fs) + fs + 10.0 * s, y + lines * fs * 1.3 + 6.0 * s))
    }

    /// 正在输入的文字、它的虚线框（选区颜色，2 实 2 空）和常亮的插入光标（文字颜色）
    pub(super) fn draw_text_edit(&self, c: &Canvas, frame: u32) {
        let (Some((e, l, caret)), Some(r)) = (self.edit_layout(), self.text_bounds()) else { return };
        let color = ui::rgb(COLORS[PEN.get().0]);
        let (lw, h) = (self.scale.round().max(1.0), self.scale.round().max(1.0) / 2.0);
        c.stroke_rect(rectf(r.left.round() + h, r.top.round() + h, r.right.round() - h, r.bottom.round() - h), ui::rgb(frame), lw, Stroke::Dash(&[2.0, 2.0]));
        c.fill_rect(caret, color);
        c.text(&l, e.at.x, e.at.y, color);
    }

    /// 正在输入时的插入光标（客户区坐标）：宿主交给 `Win::ime`，输入法的候选窗跟着它
    pub fn caret(&self, to_client: impl Fn(Pt) -> Pt) -> Option<Rect> {
        let (_, _, c) = self.edit_layout()?;
        Some(outer(client(c, &to_client)))
    }

    /// 正在输入时宿主先把键盘、输入法的事交给这里：吃掉了就返回要重画的地方（客户区坐标），不是给它的返回 None。
    /// Enter 换行，Esc 结束输入，Ctrl+V（⌘V）粘贴纯文字；别的按键不能落到工具快捷键上（打字打出 R 不能切到矩形）。
    /// `to_client` 把图像坐标换成客户区坐标（遮罩原样返回；钉图乘缩放、加描边）
    pub fn text_event(&mut self, ev: &Event, to_client: impl Fn(Pt) -> Pt) -> Option<Vec<Rect>> {
        let before = self.text_bounds();
        let e = self.text.as_mut()?;
        match ev {
            Event::Char(c) => e.insert(c.encode_utf8(&mut [0; 4])),
            Event::Ime { result, comp, cursor } => e.compose(result.as_deref(), comp.clone(), *cursor),
            Event::Key { key: Key::Esc, .. } => self.finish_text(),
            Event::Key { key: Key::Enter, .. } => e.insert("\n"),
            Event::Key { key: Key::Back, .. } => {
                if e.caret > 0 {
                    e.key(Key::Left);
                    e.key(Key::Delete);
                }
            }
            Event::Key { key: Key::Char(b'V'), mods } if mods.cmd => e.insert(&plat::paste_text().unwrap_or_default()),
            Event::Key { key, .. } => e.key(*key),
            _ => return None,
        }
        // 内容、光标变了，输入框的范围不一定变（宿主只比较范围）：前后两个框都重画
        Some([before, self.text_bounds()].into_iter().flatten().map(|r| outer(grow(client(r, &to_client), 2.0))).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_editing() {
        let mut e = Edit::default();
        e.insert("ab\r\n中文\r\tx");
        assert_eq!((e.text.as_str(), e.caret), ("ab\n中文\nx", 11));
        // 左移两格（跨过换行）插入，Home / End 只到当前行
        e.key(Key::Left);
        e.key(Key::Left);
        e.insert("字");
        assert_eq!(e.text, "ab\n中文字\nx");
        e.key(Key::Home);
        e.insert(">");
        e.key(Key::End);
        e.insert("<");
        assert_eq!(e.text, "ab\n>中文字<\nx");
        // Delete 删光标后面的；到头了左移、到尾了右移、Delete 都不动
        e.key(Key::Delete);
        assert_eq!(e.text, "ab\n>中文字<x");
        e.key(Key::End);
        e.key(Key::Right);
        e.key(Key::Delete);
        assert_eq!((e.text.len(), e.caret), (15, 15));
        e.caret = 0;
        e.key(Key::Left);
        assert_eq!(e.caret, 0);
    }

    #[test]
    fn ime_composition() {
        let mut e = Edit::default();
        e.insert("ab");
        e.key(Key::Left);
        // 组合串不进内容，光标位置夹在组合串长度里；上屏的字插在光标处
        e.compose(None, "ni'hao".into(), 99);
        assert_eq!((e.text.as_str(), e.comp_cur), ("ab", 6));
        e.compose(Some("你好"), String::new(), 0);
        assert_eq!((e.text.as_str(), e.comp.as_str(), e.caret), ("a你好b", "", 7));
    }
}
