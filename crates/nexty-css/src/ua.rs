//! HTML 渲染默认样式（UA origin 样式表）。
//!
//! 内容取自 [HTML Standard §15 Rendering](https://html.spec.whatwg.org/multipage/rendering.html)
//! 的期望渲染意图：块级元素的 `display: block`、头部的 `display: none`、
//! 标题字号/字重、强调样式与常见默认 margin。本样式表由样式层持有，
//! 经 [`crate::cascade`] 的 UA origin 参与级联（优先级低于 author 声明，
//! UA `!important` 最高）。

use std::sync::OnceLock;

use crate::parser::Stylesheet;

/// HTML UA 默认样式表（首次调用时解析一次）。
#[must_use]
pub fn html_ua_stylesheet() -> &'static Stylesheet {
    static SHEET: OnceLock<Stylesheet> = OnceLock::new();
    SHEET.get_or_init(|| Stylesheet::parse(HTML_UA_CSS))
}

const HTML_UA_CSS: &str = "
html, body, div, p, dl, dt, dd, ul, ol, section, article, aside, header,
footer, main, nav, figure, figcaption, blockquote, pre, hr, form, fieldset,
legend, address, h1, h2, h3, h4, h5, h6, table, caption, thead, tbody, tfoot,
tr, td, th, details, summary, dialog, center { display: block }

head, title, base, link, meta, style, script, noscript, template, datalist,
area, source, track, param { display: none }

li { display: list-item }

body { margin: 8px }

p, blockquote, figure, dl, ul, ol, pre, address, fieldset { margin: 1em 0 }

h1 { font-size: 2em; margin: 0.67em 0; font-weight: bold }
h2 { font-size: 1.5em; margin: 0.75em 0; font-weight: bold }
h3 { font-size: 1.17em; margin: 0.83em 0; font-weight: bold }
h4 { font-size: 1em; margin: 1em 0; font-weight: bold }
h5 { font-size: 0.83em; margin: 1.17em 0; font-weight: bold }
h6 { font-size: 0.67em; margin: 1.33em 0; font-weight: bold }

b, strong { font-weight: bold }
i, em, cite, dfn, var { font-style: italic }

pre, code, kbd, samp, tt { font-family: monospace }

center { text-align: center }
";
