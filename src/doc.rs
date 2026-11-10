//! 取字结果的版面结构。模型按一套简化的 Markdown 输出：段落（空行分段）、# 标题、``` 代码块、| 表格、
//! 单占一行的 ![图片](#n)（n 是截图里裁出的第几张，ocr.rs 换好）。这里拆成卡片能排版的块，
//! 同时拼出复制、选中用的纯文本：块之间换行，单元格之间 Tab、表格行之间换行（粘到表格软件里还是表格），图片不进纯文本。

/// 一段字是什么：决定字号和字体
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Para,
    Heading,
    Code,
}

/// 一段字和它在纯文本里的起点（UTF-16 下标，和排版的位置一致）
#[derive(Clone, PartialEq, Debug)]
pub struct Run {
    pub text: String,
    pub start: u32,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Node {
    Text(Kind, Run),
    /// 每行若干格（各行格数可能不一样）；`head` 是第一行是表头（下面跟着 |---| 分隔行）
    Table { rows: Vec<Vec<Run>>, head: bool },
    /// 截图里裁出的第几张；模型没给坐标或坐标不对时是 None，画占位框
    Image(Option<usize>),
}

#[derive(Clone, Default, Debug)]
pub struct Doc {
    /// 模型给的原样文字：翻译时发这个，结构跟着带过去
    pub src: String,
    pub plain: String,
    pub nodes: Vec<Node>,
}

impl Doc {
    /// 纯文本的行数：标题栏「3 行」
    pub fn lines(&self) -> usize {
        self.plain.lines().count().max(1)
    }
}

/// 拼纯文本，记下每段的起点
#[derive(Default)]
struct Plain {
    s: String,
    n: u32,
    /// 已经有过一块：下一块前面要换行
    started: bool,
}

impl Plain {
    fn push(&mut self, t: &str) -> Run {
        let start = self.n;
        self.s.push_str(t);
        self.n += t.encode_utf16().count() as u32;
        Run { text: t.into(), start }
    }

    fn sep(&mut self, c: char) {
        self.s.push(c);
        self.n += 1;
    }

    /// 新的一块开始
    fn block(&mut self) {
        if std::mem::replace(&mut self.started, true) {
            self.sep('\n');
        }
    }
}

/// 单占一行的 `![说明](里面)`：返回括号里的
pub fn image_ref(line: &str) -> Option<&str> {
    let rest = line.trim().strip_prefix("![")?.strip_suffix(')')?;
    Some(&rest[rest.find("](")? + 2..])
}

/// 模型偶尔还是会加粗（表头最常见）：去掉 **
fn clean(s: &str) -> String {
    s.replace("**", "")
}

/// `# 标题` 到 `###### 标题`
fn heading(t: &str) -> Option<&str> {
    let n = t.len() - t.trim_start_matches('#').len();
    (1..=6).contains(&n).then(|| t[n..].strip_prefix(' ')).flatten().map(str::trim)
}

/// 表格的一行拆成格：首尾的 | 不算，`\|` 是格里的竖线，格里的 <br> 换成空格（纯文本里一格只能一行）
fn cells(row: &str) -> Vec<String> {
    let row = row.trim();
    let body = row.strip_prefix('|').unwrap_or(row);
    let mut v = vec![String::new()];
    let mut it = body.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\\' if it.peek() == Some(&'|') => {
                v.last_mut().unwrap().push('|');
                it.next();
            }
            '|' => v.push(String::new()),
            c => v.last_mut().unwrap().push(c),
        }
    }
    if row.ends_with('|') && !row.ends_with("\\|") && v.len() > 1 {
        v.pop();
    }
    v.iter().map(|c| clean(c.replace("<br>", " ").replace("<br/>", " ").replace("<br />", " ").trim())).collect()
}

/// |---|:---:| 这样的分隔行
fn is_rule(cells: &[String]) -> bool {
    cells.iter().all(|c| c.contains('-') && c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')))
}

pub fn parse(src: &str) -> Doc {
    let mut p = Plain::default();
    let mut nodes = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let mut table: Vec<&str> = Vec::new();
    let mut code: Option<Vec<&str>> = None;

    let flush_para = |para: &mut Vec<&str>, p: &mut Plain, nodes: &mut Vec<Node>| {
        if !para.is_empty() {
            p.block();
            nodes.push(Node::Text(Kind::Para, p.push(&clean(&para.join("\n")))));
            para.clear();
        }
    };
    let flush_table = |table: &mut Vec<&str>, p: &mut Plain, nodes: &mut Vec<Node>| {
        if table.is_empty() {
            return;
        }
        let all: Vec<Vec<String>> = table.drain(..).map(cells).collect();
        let head = all.get(1).is_some_and(|r| is_rule(r));
        let mut rows = Vec::new();
        p.block();
        for r in all.into_iter().filter(|r| !is_rule(r)) {
            if !rows.is_empty() {
                p.sep('\n');
            }
            let mut row = Vec::new();
            for (i, c) in r.iter().enumerate() {
                if i > 0 {
                    p.sep('\t');
                }
                row.push(p.push(c));
            }
            rows.push(row);
        }
        nodes.push(Node::Table { rows, head });
    };

    for line in src.lines() {
        if let Some(c) = &mut code {
            if line.trim_start().starts_with("```") {
                if c.iter().any(|l| !l.is_empty()) {
                    p.block();
                    nodes.push(Node::Text(Kind::Code, p.push(&c.join("\n"))));
                }
                code = None;
            } else {
                c.push(line.trim_end());
            }
            continue;
        }
        let t = line.trim();
        if !t.starts_with('|') {
            flush_table(&mut table, &mut p, &mut nodes);
        }
        if t.starts_with('|') {
            flush_para(&mut para, &mut p, &mut nodes);
            table.push(t);
        } else if t.starts_with("```") {
            flush_para(&mut para, &mut p, &mut nodes);
            code = Some(Vec::new());
        } else if t.is_empty() {
            flush_para(&mut para, &mut p, &mut nodes);
        } else if let Some(h) = heading(t) {
            flush_para(&mut para, &mut p, &mut nodes);
            p.block();
            nodes.push(Node::Text(Kind::Heading, p.push(&clean(h))));
        } else if let Some(r) = image_ref(t) {
            flush_para(&mut para, &mut p, &mut nodes);
            nodes.push(Node::Image(r.strip_prefix('#').and_then(|n| n.parse().ok())));
        } else {
            para.push(line.trim_end());
        }
    }
    flush_table(&mut table, &mut p, &mut nodes);
    flush_para(&mut para, &mut p, &mut nodes);
    // 没收尾的代码块也算
    if let Some(c) = code.filter(|c| c.iter().any(|l| !l.is_empty())) {
        p.block();
        nodes.push(Node::Text(Kind::Code, p.push(&c.join("\n"))));
    }
    Doc { src: src.into(), plain: p.s, nodes }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(n: &Node) -> (&str, u32) {
        match n {
            Node::Text(_, r) => (&r.text, r.start),
            _ => panic!("不是文字：{n:?}"),
        }
    }

    #[test]
    fn plain_text_stays_as_is() {
        let d = parse("第一行\n第二行");
        assert_eq!(d.plain, "第一行\n第二行");
        assert_eq!(d.nodes.len(), 1);
        assert_eq!(d.lines(), 2);
    }

    /// 表格：表头、分隔行去掉、格里的竖线和 <br>、加粗；纯文本里 Tab 分格；每段的起点和纯文本对得上
    #[test]
    fn table_and_offsets() {
        let src = "# 价格表\n\n说明 **一**\n\n| 名称 | 价格 |\n|:---|---:|\n| **苹果** | 3 |\n| a\\|b | 4<br>元 |\n\n![图片](#0)\n![图片]()\n\n```rust\nfn main() {}\n    x\n```\n尾巴";
        let d = parse(src);
        assert_eq!(d.plain, "价格表\n说明 一\n名称\t价格\n苹果\t3\na|b\t4 元\nfn main() {}\n    x\n尾巴");
        assert!(matches!(d.nodes[0], Node::Text(Kind::Heading, _)));
        let Node::Table { rows, head } = &d.nodes[2] else { panic!("{:?}", d.nodes[2]) };
        assert!(*head);
        let cells: Vec<Vec<&str>> = rows.iter().map(|r| r.iter().map(|c| c.text.as_str()).collect()).collect();
        assert_eq!(cells, [["名称", "价格"], ["苹果", "3"], ["a|b", "4 元"]]);
        assert_eq!(d.nodes[3], Node::Image(Some(0)));
        assert_eq!(d.nodes[4], Node::Image(None));
        assert!(matches!(d.nodes[5], Node::Text(Kind::Code, _)));
        assert_eq!(text(&d.nodes[6]).0, "尾巴");
        // 每段在纯文本里的位置
        let u: Vec<u16> = d.plain.encode_utf16().collect();
        let at = |r: &Run| String::from_utf16(&u[r.start as usize..][..r.text.encode_utf16().count()]).unwrap();
        for n in &d.nodes {
            match n {
                Node::Text(_, r) => assert_eq!(at(r), r.text),
                Node::Table { rows, .. } => rows.iter().flatten().for_each(|r| assert_eq!(at(r), r.text)),
                Node::Image(_) => {}
            }
        }
    }

    /// 没有分隔行就没有表头；空格、行尾没有 |；没收尾的代码块
    #[test]
    fn loose_markdown() {
        let d = parse("| a | |\n| c | d\n```\nlet x = 1;");
        let Node::Table { rows, head } = &d.nodes[0] else { panic!() };
        assert!(!head);
        assert_eq!(rows.iter().map(|r| r.len()).collect::<Vec<_>>(), [2, 2]);
        assert_eq!(d.plain, "a\t\nc\td\nlet x = 1;");
        assert!(matches!(d.nodes[1], Node::Text(Kind::Code, _)));
        // 不是标题、不是图片
        let d = parse("#hashtag\n![x](a) 后面还有字");
        assert_eq!(d.nodes.len(), 1);
        assert_eq!(image_ref("![图片](12, 3,40,50)"), Some("12, 3,40,50"));
    }
}
