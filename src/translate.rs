//! OpenAI 兼容接口的请求（`chat/completions`）：翻译，和大模型取字。同步调用，跑在工作线程里。

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt;
use std::time::Duration;
use ureq::http::Uri;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

pub const TIMEOUT: Duration = Duration::from_secs(30);
/// 大模型取字：整屏的字要一个个吐出来，比翻译一段话慢
pub const OCR_TIMEOUT: Duration = Duration::from_secs(60);
/// 设置里预填的服务：DeepSeek，模型用最便宜的一档
pub const DEEPSEEK: &str = "https://api.deepseek.com";
pub const DEEPSEEK_MODEL: &str = "deepseek-flash";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Zh,
    En,
}

impl Lang {
    fn name(self) -> &'static str {
        match self {
            Lang::Zh => "简体中文",
            Lang::En => "English",
        }
    }

    /// 卡片标题栏「English → 中文」里的叫法
    pub fn label(self) -> &'static str {
        match self {
            Lang::Zh => "中文",
            Lang::En => "English",
        }
    }

    /// 卡片标题栏语言标签「英 → 中」用
    pub fn short(self) -> &'static str {
        match self {
            Lang::Zh => "中",
            Lang::En => "英",
        }
    }
}

/// 翻译服务配置。故意不 derive Debug，免得 API Key 被打进日志。
#[derive(Clone)]
pub struct Config {
    pub endpoint: String,
    pub model: String,
    /// 可以为空（本地 Ollama 不要 Key），为空时不发 Authorization
    pub api_key: String,
    pub target: Lang,
}

impl Config {
    /// 大模型取字的服务：`endpoint` 留空就用这个服务的地址和 Key，`model` 留空就用这个模型。
    /// Key 跟着地址走，单独填了地址就只用 `key`，不会把翻译的 Key 发给别家。
    pub fn overlay(&self, endpoint: &str, model: &str, key: String) -> Config {
        let own = !endpoint.is_empty();
        Config {
            endpoint: if own { endpoint.into() } else { self.endpoint.clone() },
            model: if model.is_empty() { self.model.clone() } else { model.into() },
            api_key: if own { key } else { self.api_key.clone() },
            target: self.target,
        }
    }

    /// 地址、模型都有，不是本机服务时还有 Key
    pub fn ready(&self) -> bool {
        !self.endpoint.is_empty() && !self.model.is_empty() && (!self.api_key.is_empty() || !needs_key(&self.endpoint))
    }
}

/// 翻译和测试连接共用的错误分类；Display 给出用户看得懂的中文，不含 API Key。
#[derive(Debug, PartialEq)]
pub enum Error {
    BadEndpoint,
    /// 断网、DNS 失败、连接被拒、TLS 握手失败；带主机名
    Offline(String),
    BadKey(u16),
    BadModel(String, u16),
    /// 404 但响应里没提模型，多半是地址路径写错
    NotFound,
    /// 列模型时 404：地址写错，或者这家不提供模型列表
    NoModelList,
    RateLimited(u16),
    Unavailable(u16),
    /// 带秒数
    Timeout(u64),
    BadResponse,
    /// 其他状态码，带服务返回的说明（已去掉 Key）
    Status(u16, String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::BadEndpoint => write!(f, "服务地址不对：需要以 https:// 开头（本机服务可以用 http://），请到设置里检查。"),
            Error::Offline(host) => write!(f, "连不上 {host}，请检查网络和服务地址。"),
            Error::BadKey(code) => write!(f, "API Key 无效（{code}），请到设置里检查。"),
            Error::BadModel(model, code) => write!(f, "找不到模型 {model}（{code}），请检查模型名。"),
            Error::NotFound => write!(f, "服务地址下找不到接口（404），请检查服务地址。"),
            Error::NoModelList => write!(f, "服务地址下没有模型列表（404），请检查服务地址，或者直接填写模型名。"),
            Error::RateLimited(code) => write!(f, "请求太频繁或额度已用完（{code}），请稍后重试或检查账户余额。"),
            Error::Unavailable(code) => write!(f, "服务暂时不可用（{code}），请稍后重试。"),
            Error::Timeout(secs) => write!(f, "服务 {secs} 秒内没有响应，请稍后重试。"),
            Error::BadResponse => write!(f, "服务返回的内容格式不对，请确认它是 OpenAI 兼容接口。"),
            Error::Status(code, msg) if msg.is_empty() => write!(f, "服务返回错误（{code}）。"),
            Error::Status(code, msg) => write!(f, "服务返回错误（{code}）：{msg}"),
        }
    }
}

/// 翻译一段文字，返回译文。目标语言按 `pick_target` 的规则调整。
pub fn translate(cfg: &Config, text: &str, timeout: Duration) -> Result<String, Error> {
    let to = pick_target(text, cfg.target).name();
    let system = format!(
        "你是翻译引擎。把用户发来的文字翻译成{to}。只输出译文，不要解释，不要加引号或任何前后缀。保留原文的换行和空行。\
         原文里的 Markdown 结构原样保留：表格的行、列和 | 分隔行，# 标题，列表的符号和编号，``` 代码块（代码不翻译），\
         ![图片](…) 这样的行原样照抄。"
    );
    chat(cfg, json!([{"role": "system", "content": system}, {"role": "user", "content": text}]), timeout)
}

/// 设置里的「测试连接」：发一个最小请求，错误分类和翻译相同。
pub fn test_connection(cfg: &Config, timeout: Duration) -> Result<(), Error> {
    chat(cfg, json!([{"role": "system", "content": "只回复 OK。"}, {"role": "user", "content": "ping"}]), timeout).map(drop)
}

/// 输出的格式由 doc.rs 解析，图片坐标由 ocr.rs 换成裁出的图
const OCR_PROMPT: &str = "把图片里的文字原样转写出来：逐字照抄，不翻译，不改错别字，不补全被截断的字，不加解释、标题或任何前后缀。\
按阅读顺序输出，用 Markdown 保留版面结构：表格写成 Markdown 表格，第一行是表头时下面加 |---| 分隔行，合并的单元格把字写在第一格、其余格留空；\
标题前加 #；代码放进 ``` 代码块，缩进保留；列表保留原来的符号或编号。不用加粗、斜体、链接这类行内标记。\
照片、插图、图表这类较大的图片单独占一行，写成 ![图片](左,上,右,下)，四个数是图片在这张图里的位置，左右按图宽、上下按图高各分成 0 到 1000；\
图片里的字不转写，小图标不用管。同一段文字在屏幕上自动折行的地方接起来，真正的换行（列表项、诗）保留，段落之间空一行。\
图里没有文字就什么都不输出。";

/// 大模型取字：`png` 是 base64 编码的 PNG，返回图里的文字，没有字时为空。
/// 提示词和图片放在同一条用户消息里：有的 OCR 模型不理会 system。
pub fn read_image(cfg: &Config, png: &str, timeout: Duration) -> Result<String, Error> {
    let content = json!([
        {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{png}")}},
        {"type": "text", "text": OCR_PROMPT},
    ]);
    chat(cfg, json!([{"role": "user", "content": content}]), timeout).map(|t| unfence(&t).to_string())
}

/// 32×32 的白图
const WHITE_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAIAAAD8GO2jAAAAJklEQVR42u3NMQ0AAAwDoPo33arYsQQMkB6LQCAQCAQCgUAg+BIMi1X0ptsIcT0AAAAASUVORK5CYII=";

/// 文字识别页的「测试连接」：发一张白图，模型不收图片时服务会报错
pub fn test_image(cfg: &Config, timeout: Duration) -> Result<(), Error> {
    read_image(cfg, WHITE_PNG, timeout).map(drop)
}

/// 模型偶尔会把结果整个包进 ``` 代码块：剥掉，连同 ```text 这一行；标了编程语言的是真的代码，留给 doc.rs 按代码排
fn unfence(t: &str) -> &str {
    let Some(inner) = t.strip_prefix("```").and_then(|r| r.strip_suffix("```")) else { return t };
    match inner.split_once('\n') {
        Some((lang, body)) if matches!(lang.trim(), "" | "markdown" | "md" | "text" | "txt" | "plaintext") => body.trim_end(),
        Some(_) => t,
        None => inner,
    }
}

/// 设置里的「选择…」：列出服务上的模型名，按服务给的顺序。
pub fn list_models(cfg: &Config, timeout: Duration) -> Result<Vec<String>, Error> {
    let json = call(cfg, endpoint_url(&cfg.endpoint, "models")?, None, timeout).map_err(|e| match e {
        Error::NotFound => Error::NoModelList,
        e => e,
    })?;
    let list = json["data"].as_array().ok_or(Error::BadResponse)?;
    Ok(list.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
}

/// 不是本机服务就得有 API Key；翻译卡片拿它判断「还没配置」
pub fn needs_key(endpoint: &str) -> bool {
    !endpoint_url(endpoint, "models").is_ok_and(|u| is_local(&u))
}

/// 原文已经是目标语言时换一个：目标英文且原文英文就译成中文，其余译成英文。
pub fn pick_target(text: &str, target: Lang) -> Lang {
    match (detect(text), target) {
        (Some(Lang::En), Lang::En) => Lang::Zh,
        (Some(src), t) if src == t => Lang::En,
        _ => target,
    }
}

/// 英文常见虚词
const EN_WORDS: &[&str] = &[
    "the", "and", "is", "are", "was", "were", "of", "to", "in", "on", "at", "for", "with", "from", "by", "this", "that", "it", "you", "we", "they", "be",
    "have", "has", "not", "or", "will", "can", "an", "as", "if", "but", "your", "my", "i",
];
/// 法、德、西、意、葡、荷文的常见虚词（英文里不当词用的）
const OTHER_WORDS: &[&str] = &[
    "le", "la", "les", "des", "du", "un", "une", "est", "et", "sont", "pas", "pour", "dans", "sur", "avec", "ce", "cette", "je", "vous", "nous", "qui", "que",
    "au", "aux", "mais", "ou", "de", "en", "der", "das", "und", "ist", "nicht", "ein", "eine", "ich", "mit", "auf", "zu", "dem", "den", "auch", "von", "sich",
    "wir", "sind", "es", "el", "los", "las", "y", "una", "por", "con", "para", "del", "como", "pero", "muy", "se", "lo", "il", "di", "che", "della", "gli",
    "sono", "os", "em", "uma", "com", "het", "een", "van", "niet", "zijn", "met",
];

/// 粗判原文语言：汉字和假名每个算一份，单词每个算一份。汉字占一半及以上且没有假名才算中文；
/// 否则看单词：英文虚词不比别的语言的线索（上面那些虚词、带非 ASCII 字母的词，比如重音字母、韩文、俄文）少就算英文。
/// 别的语言和没有字都返回 None：Jie 只译成中英，认不出的照设好的目标语言译，卡片上不写原文是哪种语言。
pub fn detect(text: &str) -> Option<Lang> {
    let cjk = |c: char| {
        matches!(c, '\u{3041}'..='\u{3096}' | '\u{30A1}'..='\u{30FA}' | '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}')
    };
    let kana = text.chars().filter(|c| matches!(c, '\u{3041}'..='\u{3096}' | '\u{30A1}'..='\u{30FA}')).count();
    let han = text.chars().filter(|&c| cjk(c)).count() - kana;
    let (mut words, mut en, mut other) = (0, 0, 0);
    for w in text.split(|c: char| !c.is_alphabetic() || cjk(c)).filter(|w| !w.is_empty()) {
        words += 1;
        let w = w.to_lowercase();
        if !w.is_ascii() || OTHER_WORDS.contains(&w.as_str()) {
            other += 1;
        } else if EN_WORDS.contains(&w.as_str()) {
            en += 1;
        }
    }
    if han + kana + words == 0 {
        None
    } else if han + kana >= words {
        // 含假名的原文不算中文，避免自动切换到英文目标。
        (kana == 0).then_some(Lang::Zh)
    } else {
        (en >= other).then_some(Lang::En)
    }
}

/// 按 OpenAI 兼容约定拼出 `…/chat/completions`、`…/models`，并检查只用 HTTPS（本机地址例外）。
fn endpoint_url(endpoint: &str, path: &str) -> Result<Uri, Error> {
    let base = endpoint.trim().trim_end_matches('/');
    let base = base.strip_suffix("/chat/completions").unwrap_or(base);
    let uri: Uri = format!("{base}/{path}").parse().map_err(|_| Error::BadEndpoint)?;
    match uri.scheme_str() {
        Some("https") if uri.host().is_some_and(|h| !h.is_empty()) => Ok(uri),
        Some("http") if is_local(&uri) => Ok(uri),
        _ => Err(Error::BadEndpoint),
    }
}

/// 本机服务（Ollama 这类）：可以用 http，可以不要 Key
fn is_local(uri: &Uri) -> bool {
    matches!(uri.host().unwrap_or_default().to_ascii_lowercase().as_str(), "localhost" | "127.0.0.1" | "[::1]")
}

fn chat(cfg: &Config, messages: Value, timeout: Duration) -> Result<String, Error> {
    let uri = endpoint_url(&cfg.endpoint, "chat/completions")?;
    let body = chat_body(&uri, &cfg.model, messages);
    call(cfg, uri, Some(body), timeout)?["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.trim().to_string())
        .ok_or(Error::BadResponse)
}

fn chat_body(uri: &Uri, model: &str, messages: Value) -> Value {
    let mut body = json!({"model": model, "messages": messages});
    // DeepSeek V4 默认开思考，翻译、取字都用不上，开着又慢又多花钱；别家不带（OpenAI 会拒收不认识的参数）
    if uri.host().is_some_and(|h| h.eq_ignore_ascii_case("api.deepseek.com")) {
        body["thinking"] = json!({"type": "disabled"});
    }
    body
}

/// 发一次请求：`body` 为 None 时 GET。2xx 返回响应 JSON（不是 JSON 时是 Null），其余按状态码分类。
fn call(cfg: &Config, uri: Uri, body: Option<Value>, timeout: Duration) -> Result<Value, Error> {
    let host = uri.host().unwrap_or_default().to_string();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        // 不跟随跳转：免得被跳到 http 地址，把原文明文发出去
        .max_redirects(0)
        // 用系统的 schannel 和系统证书库，exe 里不用带 TLS 实现和根证书
        .tls_config(
            TlsConfig::builder()
                .provider(TlsProvider::NativeTls)
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into();
    let auth = format!("Bearer {}", cfg.api_key);
    let net = |e: ureq::Error| match e {
        ureq::Error::Timeout(_) => Error::Timeout(timeout.as_secs()),
        _ => Error::Offline(host.clone()),
    };
    let chat = body.is_some();
    let sent = match body {
        Some(b) => {
            let mut req = agent.post(uri).content_type("application/json");
            if !cfg.api_key.is_empty() {
                req = req.header("Authorization", &auth);
            }
            req.send(b.to_string())
        }
        None => {
            let mut req = agent.get(uri);
            if !cfg.api_key.is_empty() {
                req = req.header("Authorization", &auth);
            }
            req.call()
        }
    };
    let mut resp = sent.map_err(net)?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().map_err(net)?;
    let json: Value = serde_json::from_str(&text).unwrap_or_default();
    match status {
        200..=299 => Ok(json),
        401 | 403 => Err(Error::BadKey(status)),
        // DeepSeek 余额不足回 402
        402 | 429 => Err(Error::RateLimited(status)),
        400..=499 if chat && no_such_model(&text) => Err(Error::BadModel(cfg.model.clone(), status)),
        404 => Err(Error::NotFound),
        500.. => Err(Error::Unavailable(status)),
        _ => Err(Error::Status(status, server_message(&json, &text, &cfg.api_key))),
    }
}

/// 各家「模型不存在」的说法：OpenAI/通义 model_not_found、DeepSeek Model Not Exist、
/// Ollama model "x" not found、Kimi Not found the model、智谱 模型不存在
fn no_such_model(body: &str) -> bool {
    let b = body.to_lowercase();
    b.contains("model_not_found")
        || b.contains("模型不存在")
        || (b.contains("model") && (b.contains("not exist") || b.contains("not found")))
}

/// 取服务返回的说明，截短，并把 Key 抹掉（有的服务会在报错里回显 Key）
fn server_message(json: &Value, raw: &str, key: &str) -> String {
    let msg = json["error"]["message"]
        .as_str()
        .or(json["error"].as_str())
        .or(json["message"].as_str())
        .unwrap_or(raw);
    let msg: String = msg.trim().chars().take(200).collect();
    if key.is_empty() { msg } else { msg.replace(key, "***") }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::thread;

    const KEY: &str = "sk-test-secret-123";

    fn cfg(endpoint: &str) -> Config {
        Config { endpoint: endpoint.into(), model: "m1".into(), api_key: KEY.into(), target: Lang::Zh }
    }

    pub(crate) fn http(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    /// 本地 mock：每个连接读完请求后都回 `reply`；None 表示接一个连接后一直不回应，直到客户端断开。
    /// 返回服务地址（带 /v1）和收到的请求原文，按到达顺序。
    pub(crate) fn mock(reply: Option<String>) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for s in listener.incoming() {
                let mut s = s.unwrap();
                let _ = tx.send(read_request(&mut s));
                match &reply {
                    Some(r) => s.write_all(r.as_bytes()).unwrap(),
                    None => return drop(s.read(&mut [0u8; 1])),
                }
            }
        });
        (base, rx)
    }

    fn read_request(s: &mut TcpStream) -> String {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = s.read(&mut chunk).unwrap();
            buf.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buf).to_lowercase();
            if let Some(end) = text.find("\r\n\r\n") {
                let len = text
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .map_or(0, |v| v.trim().parse().unwrap());
                if buf.len() >= end + 4 + len {
                    return String::from_utf8(buf).unwrap();
                }
            }
            assert!(n > 0, "请求没读完连接就断了");
        }
    }

    /// 发一次翻译，返回结果和 mock 收到的请求
    fn run(reply: String) -> (Result<String, Error>, String) {
        let (base, rx) = mock(Some(reply));
        let r = translate(&cfg(&base), "Hello\nworld", Duration::from_secs(5));
        (r, rx.recv().unwrap())
    }

    #[test]
    fn success() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"你好\n世界\n"}}]}"#;
        let (r, req) = run(http("200 OK", body));
        assert_eq!(r.unwrap(), "你好\n世界");
        assert!(req.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"), "{req}");
        assert!(req.to_lowercase().contains(&format!("authorization: bearer {KEY}")));
        let json: Value = serde_json::from_str(req.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(json["model"], "m1");
        assert!(json["messages"][0]["content"].as_str().unwrap().contains("简体中文"));
        assert_eq!(json["messages"][1]["content"], "Hello\nworld");
        assert!(json.get("thinking").is_none());
    }

    #[test]
    fn bad_key_401_hides_key() {
        let body = format!(r#"{{"error":{{"message":"Incorrect API key provided: {KEY}"}}}}"#);
        let e = run(http("401 Unauthorized", &body)).0.unwrap_err();
        assert_eq!(e, Error::BadKey(401));
        assert_eq!(e.to_string(), "API Key 无效（401），请到设置里检查。");
    }

    #[test]
    fn bad_model_404() {
        let body = r#"{"error":{"message":"The model `m1` does not exist","code":"model_not_found"}}"#;
        let e = run(http("404 Not Found", body)).0.unwrap_err();
        assert_eq!(e, Error::BadModel("m1".into(), 404));
        assert_eq!(e.to_string(), "找不到模型 m1（404），请检查模型名。");
        // DeepSeek 模型名不对回 400
        let body = r#"{"error":{"message":"Model Not Exist","type":"invalid_request_error"}}"#;
        assert_eq!(run(http("400 Bad Request", body)).0, Err(Error::BadModel("m1".into(), 400)));
        // 路径写错的 404 不提模型
        assert_eq!(run(http("404 Not Found", "404 page not found")).0, Err(Error::NotFound));
    }

    #[test]
    fn other_statuses() {
        assert_eq!(run(http("429 Too Many Requests", "{}")).0, Err(Error::RateLimited(429)));
        assert_eq!(run(http("402 Payment Required", "{}")).0, Err(Error::RateLimited(402)));
        assert_eq!(run(http("503 Service Unavailable", "")).0, Err(Error::Unavailable(503)));
        assert_eq!(run(http("200 OK", "<html>")).0, Err(Error::BadResponse));
        let body = format!(r#"{{"error":{{"message":"bad input, key {KEY}"}}}}"#);
        let e = run(http("400 Bad Request", &body)).0.unwrap_err();
        assert_eq!(e.to_string(), "服务返回错误（400）：bad input, key ***");
    }

    #[test]
    fn no_key_no_auth_header() {
        let (base, rx) = mock(Some(http("200 OK", r#"{"choices":[{"message":{"content":"OK"}}]}"#)));
        let c = Config { api_key: String::new(), ..cfg(&base) };
        assert_eq!(test_connection(&c, Duration::from_secs(5)), Ok(()));
        assert!(!rx.recv().unwrap().to_lowercase().contains("authorization"));
    }

    #[test]
    fn timeout() {
        let (base, _rx) = mock(None);
        let r = translate(&cfg(&base), "hi", Duration::from_millis(300));
        assert_eq!(r, Err(Error::Timeout(0)));
        assert_eq!(Error::Timeout(30).to_string(), "服务 30 秒内没有响应，请稍后重试。");
    }

    #[test]
    fn connection_refused() {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let e = test_connection(&cfg(&format!("http://127.0.0.1:{port}/v1")), Duration::from_secs(10));
        assert_eq!(e, Err(Error::Offline("127.0.0.1".into())));
        assert_eq!(e.unwrap_err().to_string(), "连不上 127.0.0.1，请检查网络和服务地址。");
    }

    #[test]
    fn endpoint() {
        let ok = |s: &str| endpoint_url(s, "chat/completions").unwrap().to_string();
        assert_eq!(ok("https://api.deepseek.com"), "https://api.deepseek.com/chat/completions");
        assert_eq!(ok("https://x.com/v1/"), "https://x.com/v1/chat/completions");
        assert_eq!(ok(" https://x.com/v1/chat/completions/ "), "https://x.com/v1/chat/completions");
        assert_eq!(ok("HTTPS://X.com/v4"), "https://X.com/v4/chat/completions");
        assert_eq!(ok("http://localhost:11434/v1"), "http://localhost:11434/v1/chat/completions");
        assert_eq!(ok("http://127.0.0.1:11434/v1"), "http://127.0.0.1:11434/v1/chat/completions");
        assert_eq!(ok("http://[::1]:11434/v1"), "http://[::1]:11434/v1/chat/completions");
        assert_eq!(endpoint_url("https://x.com/v1/chat/completions", "models").unwrap().to_string(), "https://x.com/v1/models");
        for bad in [
            "",
            "api.deepseek.com",
            "http://api.deepseek.com",
            "http://localhost.evil.com/v1",
            "http://localhost@evil.com/v1",
            "ftp://x.com",
            "https://",
            "https://a b.com",
        ] {
            assert_eq!(endpoint_url(bad, "chat/completions"), Err(Error::BadEndpoint), "{bad}");
        }
        // 地址不合规时直接报错，不发请求
        assert_eq!(translate(&cfg("http://example.com"), "hi", TIMEOUT), Err(Error::BadEndpoint));
        assert_eq!(list_models(&cfg("http://example.com"), TIMEOUT), Err(Error::BadEndpoint));

        assert!(needs_key(DEEPSEEK) && needs_key("") && needs_key("http://example.com"));
        assert!(!needs_key("http://localhost:11434/v1") && !needs_key("https://127.0.0.1/v1"));
    }

    #[test]
    fn thinking_off_only_for_deepseek() {
        let body = |ep: &str| chat_body(&endpoint_url(ep, "chat/completions").unwrap(), "m", json!([]));
        assert_eq!(body(DEEPSEEK)["thinking"], json!({"type": "disabled"}));
        assert_eq!(body("https://API.DeepSeek.com/v1/")["thinking"]["type"], "disabled");
        for other in ["https://api.openai.com/v1", "https://dashscope.aliyuncs.com/compatible-mode/v1", "https://deepseek.com.evil.io/v1"] {
            assert!(body(other).get("thinking").is_none(), "{other}");
        }
    }

    #[test]
    fn models() {
        let body = r#"{"object":"list","data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"},{"id":"deepseek-v4-pro","object":"model"}]}"#;
        let (base, rx) = mock(Some(http("200 OK", body)));
        assert_eq!(list_models(&cfg(&base), Duration::from_secs(5)).unwrap(), ["deepseek-flash", "deepseek-v4-pro"]);
        let req = rx.recv().unwrap();
        assert!(req.starts_with("GET /v1/models HTTP/1.1\r\n"), "{req}");
        assert!(req.to_lowercase().contains(&format!("authorization: bearer {KEY}")));

        let list = |reply: String| {
            let (base, _rx) = mock(Some(reply));
            list_models(&cfg(&base), Duration::from_secs(5))
        };
        assert_eq!(list(http("401 Unauthorized", r#"{"error":{"message":"bad key"}}"#)), Err(Error::BadKey(401)));
        // 列模型的 404 不当成模型名错，哪怕响应里提到 model
        let e = list(http("404 Not Found", r#"{"error":{"message":"model list not found"}}"#)).unwrap_err();
        assert_eq!(e, Error::NoModelList);
        assert_eq!(e.to_string(), "服务地址下没有模型列表（404），请检查服务地址，或者直接填写模型名。");
        assert_eq!(list(http("200 OK", "<html>")), Err(Error::BadResponse));
    }

    #[test]
    fn read_image_request() {
        let body = r#"{"choices":[{"message":{"content":"```text\n第一行\n    缩进\n```"}}]}"#;
        let (base, rx) = mock(Some(http("200 OK", body)));
        assert_eq!(read_image(&cfg(&base), "QUJD", Duration::from_secs(5)).unwrap(), "第一行\n    缩进");
        let req = rx.recv().unwrap();
        let json: Value = serde_json::from_str(req.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let msg = &json["messages"][0];
        assert_eq!((json["messages"].as_array().unwrap().len(), msg["role"].as_str()), (1, Some("user")));
        assert_eq!(msg["content"][0]["image_url"]["url"], "data:image/png;base64,QUJD");
        assert!(msg["content"][1]["text"].as_str().unwrap().contains("逐字照抄"));

        // 没有字：空字符串，不是错误
        let (base, _rx) = mock(Some(http("200 OK", r#"{"choices":[{"message":{"content":""}}]}"#)));
        assert_eq!(read_image(&cfg(&base), "QUJD", Duration::from_secs(5)).unwrap(), "");
        // 模型不收图片：服务的说明原样给出来
        let (base, _rx) = mock(Some(http("400 Bad Request", r#"{"error":{"message":"image input is not supported"}}"#)));
        assert_eq!(test_image(&cfg(&base), Duration::from_secs(5)), Err(Error::Status(400, "image input is not supported".into())));
    }

    #[test]
    fn fences() {
        assert_eq!(unfence("```\na\nb\n```"), "a\nb");
        assert_eq!(unfence("```python\ndef f():\n    pass\n```"), "```python\ndef f():\n    pass\n```");
        assert_eq!(unfence("```markdown\n| a | b |\n```"), "| a | b |");
        assert_eq!(unfence("```一行```"), "一行");
        assert_eq!(unfence("a ``` b"), "a ``` b");
        assert_eq!(unfence("```rust\nfn main() {}"), "```rust\nfn main() {}");
    }

    #[test]
    fn ocr_service() {
        let tr = Config { endpoint: DEEPSEEK.into(), model: "deepseek-flash".into(), api_key: "sk-tr".into(), target: Lang::Zh };
        // 全留空：和翻译一样
        let c = tr.overlay("", "", "sk-ocr".into());
        assert_eq!((c.endpoint.as_str(), c.model.as_str(), c.api_key.as_str()), (DEEPSEEK, "deepseek-flash", "sk-tr"));
        // 只换模型：地址和 Key 还是翻译的
        let c = tr.overlay("", "deepseek-vl", "sk-ocr".into());
        assert_eq!((c.endpoint.as_str(), c.model.as_str(), c.api_key.as_str()), (DEEPSEEK, "deepseek-vl", "sk-tr"));
        // 换了地址：Key 只用自己的，哪怕是空的
        let qwen = "https://dashscope.aliyuncs.com/compatible-mode/v1";
        let c = tr.overlay(qwen, "qwen-vl-ocr", "sk-ocr".into());
        assert_eq!((c.endpoint.as_str(), c.model.as_str(), c.api_key.as_str()), (qwen, "qwen-vl-ocr", "sk-ocr"));
        let c = tr.overlay(qwen, "qwen-vl-ocr", String::new());
        assert!(c.api_key.is_empty() && !c.ready());
        assert!(tr.ready() && !tr.overlay("", "", String::new()).api_key.is_empty());
        assert!(tr.overlay("http://localhost:11434/v1", "llava", String::new()).ready());
    }

    #[test]
    fn language() {
        assert_eq!(detect("你好，世界"), Some(Lang::Zh));
        assert_eq!(detect("Hello, world"), Some(Lang::En));
        assert_eq!(detect("使用 cargo build 命令编译 Rust 项目"), Some(Lang::Zh));
        assert_eq!(detect("The word 你好 means hello"), Some(Lang::En));
        // 日语不标语言，也不能误判成中文。
        assert_eq!(detect("今日はいい天気ですね"), None);
        assert_eq!(detect("こんにちは"), None);
        assert_eq!(detect("12345 !!"), None);
        assert_eq!(detect("안녕하세요"), None);
        // 界面上没有虚词的英文、英文里夹一个重音词，还是英文
        assert_eq!(detect("File Edit Selection View Go Run Terminal Help"), Some(Lang::En));
        assert_eq!(detect("I went to the café"), Some(Lang::En));
        // 法、德、西文（不带重音也认得出）、夹英文的韩文俄文：不是英文
        assert_eq!(detect("Le chat est sur la table"), None);
        assert_eq!(detect("Der Hund ist nicht im Haus"), None);
        assert_eq!(detect("El perro está en la casa"), None);
        assert_eq!(detect("안녕하세요 Jie 입니다"), None);
        assert_eq!(detect("Привет мир Jie"), None);

        assert_eq!(pick_target("Hello", Lang::Zh), Lang::Zh);
        assert_eq!(pick_target("你好", Lang::Zh), Lang::En);
        assert_eq!(pick_target("Hello", Lang::En), Lang::Zh);
        assert_eq!(pick_target("你好", Lang::En), Lang::En);
        // 日语原文仍按所选的中、英文目标翻译。
        assert_eq!(pick_target("こんにちは", Lang::Zh), Lang::Zh);
        assert_eq!(pick_target("こんにちは", Lang::En), Lang::En);
        assert_eq!(pick_target("今日はいい天気ですね", Lang::Zh), Lang::Zh);
        assert_eq!(pick_target("12345", Lang::Zh), Lang::Zh);
        // 法文译成英文，不再被当成英文改译中文
        assert_eq!(pick_target("Le chat est sur la table", Lang::En), Lang::En);
    }
}
