//! Convert a Cosense page (line-based text) into Markdown.
//!
//! Cosense's API returns pages as a list of `{ text: String }` lines. We join the
//! lines back into a single document and parse it once with `cosy::parse()`, which
//! recognises multi-line block constructs (`code:` blocks, `table:`, etc.). The
//! resulting AST is then walked here to emit Markdown.

use cosy::ast::{Block, BlockContent, CodeBlockMeta, Link, Node, ShellPrompt};

pub fn render(project: &str, lines: &[String]) -> String {
    // Join all lines and let cosy parse the full document so multi-line blocks
    // (code:, table:) are recognized correctly.
    let input = lines.join("\n");
    let doc = match cosy::parse(&input, &()) {
        Ok(d) => d,
        Err(_) => return input, // fall back to raw text
    };

    let mut out = String::new();
    for block in doc.0.iter() {
        render_block(block, project, &mut out);
        out.push('\n');
    }
    // Trim trailing whitespace lines.
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

fn render_block(block: &Block<()>, project: &str, out: &mut String) {
    let indent = block.indent;
    match &block.content {
        BlockContent::Line(nodes) => {
            // Detect heading: a line whose first node is `Decoration { decos: "*..." }` and there
            // are no other non-whitespace nodes around it.
            if let Some(level) = detect_heading_level(nodes) {
                out.push_str(&"#".repeat(level));
                out.push(' ');
                // Inner of the heading decoration is rendered as plain inline.
                if let Some(Node::Decoration { nodes: inner, .. }) = nodes.first() {
                    render_nodes(inner, project, out);
                }
                return;
            }

            if indent > 0 {
                // Indented line → bullet list item.
                out.push_str(&"  ".repeat(indent - 1));
                out.push_str("- ");
            }
            render_nodes(nodes, project, out);
        }
        BlockContent::CodeBlock { meta, content } => {
            let lang = match meta {
                CodeBlockMeta::None => String::new(),
                CodeBlockMeta::NameOrLang(s) => infer_lang(s).to_string(),
                CodeBlockMeta::Both { language, .. } => language.clone(),
                _ => String::new(),
            };
            out.push_str("```");
            out.push_str(&lang);
            out.push('\n');
            out.push_str(content);
            if !content.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("```");
        }
        BlockContent::Table { name, rows } => {
            render_table(name, rows, project, out);
        }
        BlockContent::Quote(nodes) => {
            out.push_str("> ");
            render_nodes(nodes, project, out);
        }
        BlockContent::Helpfeel(s) => {
            out.push_str("> ? ");
            out.push_str(s);
        }
        BlockContent::CommandLine { prompt, command } => {
            let p = match prompt {
                ShellPrompt::Dollar => "$",
                ShellPrompt::Percent => "%",
                _ => "$",
            };
            out.push_str("```sh\n");
            out.push_str(p);
            out.push(' ');
            out.push_str(command);
            out.push_str("\n```");
        }
        BlockContent::Custom(_) => {}
        _ => {}
    }
}

fn detect_heading_level(nodes: &[Node<()>]) -> Option<usize> {
    // Cosense uses [* x] / [** x] / [*** x] for size-based emphasis used as headings.
    // Treat a line that consists of a single Decoration whose decos starts with '*' as a heading.
    if nodes.len() != 1 {
        return None;
    }
    if let Node::Decoration { decos, .. } = &nodes[0] {
        if !decos.is_empty() && decos.chars().all(|c| c == '*') {
            return Some(decos.len().min(6));
        }
    }
    None
}

fn render_nodes(nodes: &[Node<()>], project: &str, out: &mut String) {
    for n in nodes {
        render_node(n, project, out);
    }
}

fn render_node(node: &Node<()>, project: &str, out: &mut String) {
    match node {
        Node::Text(s) => out.push_str(s),
        Node::Link(l) => render_link(l, project, out),
        Node::Image(url) => {
            out.push_str("![](");
            out.push_str(url.as_str());
            out.push(')');
        }
        Node::LinkedImage { src, href } => {
            out.push_str("[![](");
            out.push_str(src.as_str());
            out.push_str(")](");
            out.push_str(href.as_str());
            out.push(')');
        }
        Node::Icon { name, count } => {
            for _ in 0..(*count).max(1) {
                out.push(':');
                out.push_str(name);
                out.push(':');
            }
        }
        Node::InlineCode(s) => {
            out.push('`');
            out.push_str(s);
            out.push('`');
        }
        Node::Math(s) => {
            out.push('$');
            out.push_str(s);
            out.push('$');
        }
        Node::Hashtag(s) => {
            out.push('#');
            out.push_str(s);
        }
        Node::Decoration { decos, nodes } => {
            let (open, close) = decoration_marks(decos);
            out.push_str(open);
            render_nodes(nodes, project, out);
            out.push_str(close);
        }
        Node::Strong(nodes) => {
            out.push_str("**");
            render_nodes(nodes, project, out);
            out.push_str("**");
        }
        Node::Coordinate {
            latitude,
            longitude,
            zoom: _,
        } => {
            // Render as plain text; geographic embeds aren't a Markdown primitive.
            use cosy::ast::{Latitude, Longitude};
            let (lat_dir, lat_v) = match latitude {
                Latitude::North(v) => ('N', *v),
                Latitude::South(v) => ('S', *v),
            };
            let (lon_dir, lon_v) = match longitude {
                Longitude::East(v) => ('E', *v),
                Longitude::West(v) => ('W', *v),
            };
            out.push_str(&format!("({lat_dir}{lat_v},{lon_dir}{lon_v})"));
        }
        Node::Custom(_) => {}
        _ => {}
    }
}

fn decoration_marks(decos: &str) -> (&'static str, &'static str) {
    // Cosense decorations: '*' bold (also size emphasis), '/' italic, '-' strike, '_' underline.
    // We map combinations conservatively.
    let has_star = decos.contains('*');
    let has_slash = decos.contains('/');
    let has_dash = decos.contains('-');
    match (has_star, has_slash, has_dash) {
        (true, true, _) => ("***", "***"),
        (true, false, _) => ("**", "**"),
        (false, true, _) => ("*", "*"),
        (false, false, true) => ("~~", "~~"),
        _ => ("", ""),
    }
}

fn render_link(link: &Link<()>, project: &str, out: &mut String) {
    match link {
        Link::Page(title) => {
            out.push('[');
            out.push_str(title);
            out.push_str("](");
            out.push_str(&page_url(project, title));
            out.push(')');
        }
        Link::Url(url) => {
            out.push('<');
            out.push_str(url.as_str());
            out.push('>');
        }
        Link::WithLabel { href, label } => {
            out.push('[');
            render_nodes(label, project, out);
            out.push_str("](");
            out.push_str(href.as_str());
            out.push(')');
        }
        Link::Project(p) => {
            out.push('[');
            out.push_str(p);
            out.push_str("](https://scrapbox.io/");
            out.push_str(&urlencoding::encode(p));
            out.push_str(")");
        }
        Link::ProjectPage { project: p, page } => {
            out.push('[');
            out.push_str(p);
            out.push('/');
            out.push_str(page);
            out.push_str("](https://scrapbox.io/");
            out.push_str(&urlencoding::encode(p));
            out.push('/');
            out.push_str(&urlencoding::encode(page));
            out.push(')');
        }
        _ => {}
    }
}

fn page_url(project: &str, title: &str) -> String {
    format!(
        "https://scrapbox.io/{}/{}",
        urlencoding::encode(project),
        urlencoding::encode(title)
    )
}

fn render_table(name: &str, rows: &[Vec<Vec<Node<()>>>], project: &str, out: &mut String) {
    out.push_str("**");
    out.push_str(name);
    out.push_str("**\n");
    if rows.is_empty() {
        return;
    }
    let max_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if max_cols == 0 {
        return;
    }
    // First row as header.
    write_row(&rows[0], max_cols, project, out);
    out.push('\n');
    out.push('|');
    for _ in 0..max_cols {
        out.push_str(" --- |");
    }
    if rows.len() > 1 {
        for row in &rows[1..] {
            out.push('\n');
            write_row(row, max_cols, project, out);
        }
    }
}

fn write_row(row: &[Vec<Node<()>>], max_cols: usize, project: &str, out: &mut String) {
    out.push('|');
    for i in 0..max_cols {
        out.push(' ');
        if let Some(cell) = row.get(i) {
            render_nodes(cell, project, out);
        }
        out.push_str(" |");
    }
}

fn infer_lang(name_or_lang: &str) -> &str {
    // If it looks like a filename, try to extract extension; else treat as language id.
    if let Some(idx) = name_or_lang.rfind('.') {
        &name_or_lang[idx + 1..]
    } else {
        name_or_lang
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(input: &str) -> String {
        let lines: Vec<String> = input.lines().map(str::to_owned).collect();
        render("proj", &lines)
    }

    #[test]
    fn plain_line_passes_through() {
        assert_eq!(r("hello world"), "hello world\n");
    }

    #[test]
    fn headings_use_size_decoration() {
        assert_eq!(r("[* h1]"), "# h1\n");
        assert_eq!(r("[** h2]"), "## h2\n");
        assert_eq!(r("[*** h3]"), "### h3\n");
    }

    #[test]
    fn page_link_renders_with_project_url() {
        let out = r("see [Some Page] for details");
        assert!(
            out.contains("[Some Page](https://scrapbox.io/proj/Some%20Page)"),
            "got: {out}"
        );
    }

    #[test]
    fn url_with_label() {
        let out = r("[https://example.com Example]");
        assert!(out.contains("[Example](https://example.com"), "got: {out}");
    }

    #[test]
    fn bare_url_in_brackets_renders_as_autolink() {
        let out = r("[https://example.com]");
        assert!(out.contains("<https://example.com"), "got: {out}");
    }

    #[test]
    fn indented_lines_become_bullet_list() {
        let out = r(" first\n  second");
        assert!(out.contains("- first"), "got: {out}");
        assert!(out.contains("  - second"), "got: {out}");
    }

    #[test]
    fn code_block_with_filename_uses_extension_as_lang() {
        let input = "code:main.rs\n fn main() {}\n";
        let out = r(input);
        assert!(out.contains("```rs"), "expected opening ```rs in: {out}");
        assert!(out.contains("fn main()"), "expected body content in: {out}");
        // Two fences expected: one opening (```rs) and one closing (```).
        assert_eq!(out.matches("```").count(), 2, "expected exactly 2 fences in: {out}");
    }

    #[test]
    fn code_block_without_metadata_emits_blank_lang() {
        // `code:` with no filename/lang should still produce a fenced block.
        let input = "code:\n some content\n";
        let out = r(input);
        assert!(out.contains("```\n") || out.contains("```"), "got: {out}");
        assert_eq!(out.matches("```").count(), 2, "expected exactly 2 fences in: {out}");
    }

    #[test]
    fn empty_input_produces_empty_output() {
        assert_eq!(render("p", &[]), "");
    }

    #[test]
    fn table_renders_with_pipe_syntax() {
        // Cosense table: `table:Name` followed by space-indented, tab-separated rows.
        let input = "table:Stats\n  header1\theader2\n  row1a\trow1b\n";
        let out = r(input);
        assert!(out.contains("**Stats**"), "expected table name as bold in: {out}");
        assert!(out.contains("|"), "expected pipe syntax in: {out}");
        assert!(out.contains("---"), "expected separator row in: {out}");
    }

    #[test]
    fn strikethrough_decoration() {
        let out = r("[- gone]");
        assert!(out.contains("~~gone~~"), "got: {out}");
    }

    #[test]
    fn bold_italic_combination() {
        let out = r("[*/ emph]");
        assert!(out.contains("***emph***"), "got: {out}");
    }

    #[test]
    fn heading_detection_only_when_decoration_is_pure_stars() {
        // [*- x] = bold + strike, NOT a heading
        let out = r("[*- x]");
        assert!(!out.starts_with("# "), "got: {out}");
    }

    #[test]
    fn infer_lang_extracts_extension() {
        assert_eq!(infer_lang("main.rs"), "rs");
        assert_eq!(infer_lang("rust"), "rust");
        assert_eq!(infer_lang("foo.tar.gz"), "gz");
    }
}
