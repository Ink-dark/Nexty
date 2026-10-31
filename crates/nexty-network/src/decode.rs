//! 主文档字符编码探测与解码（feature `decode`）。
//!
//! 优先级链（本轮 goal 固定）：HTTP header `Content-Type` 的 `charset` 参数、
//! BOM（UTF-8 / UTF-16LE / UTF-16BE）、`<meta charset>` / http-equiv 预扫
//! （前 [`MAX_PRESCAN_BYTES`] 字节）、默认 UTF-8，取第一个命中的层。
//! 编码标签 → 解码器由 `encoding_rs` 提供（WHATWG Encoding Standard 的
//! Firefox 实现）；本模块不引入编码自动探测（chardetng 类），样式表不在本层解码。
//!
//! 与 WHATWG HTML §13.2.3「Determining the character encoding」的偏差：不实现
//! 用户显式覆盖、父浏览上下文继承、自动探测等步骤；预扫是规范 §13.2.3.5 的
//! 简化档（见 [`prescan_charset`]）。
//!
//! 已知偏差：HTTP charset 层按「无 BOM 处理」解码——`Encoding::decode` 的 BOM
//! 嗅探会覆盖所选编码，违反 HTTP > BOM 的优先级——解码后剥掉前导 U+FEFF，使
//! BOM 字节不进入文本；若 HTTP 指定的编码与 BOM 编码族不同（如 charset=GBK 而
//! 字节流带 UTF-8 BOM），BOM 字节按该编码解出为杂字，不做特殊处理。

use encoding_rs::{Encoding, UTF_8};

/// meta 预扫只看前多少字节（WHATWG HTML §13.2.3.5 prescan 的上限）。
pub const MAX_PRESCAN_BYTES: usize = 1024;

/// 一次解码的结果。
///
/// `encoding` 是实际采用的编码名（encoding_rs 的规范名，如 `UTF-8`、`GBK`、
/// `UTF-16LE`），供样式表等消费方按文档编码降级。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedDocument {
    /// 解码后的文本；非法字节按 WHATWG Encoding 的替换语义兜底，不会失败。
    pub text: String,
    /// 实际采用的编码名。
    pub encoding: &'static str,
}

/// 按四层优先级把响应体字节解码为文本。
///
/// `content_type` 传响应 `Content-Type` 头的原始值（可为空）。解码永不失败：
/// 未知编码标签视同该层未命中落到下一层；非法字节序列由替换字符兜底。
///
/// ```
/// use nexty_network::decode_document;
///
/// let doc = decode_document(Some("text/html; charset=gbk"), &[0xD6, 0xD0]);
/// assert_eq!(doc.text, "中");
/// assert_eq!(doc.encoding, "GBK");
/// ```
pub fn decode_document(content_type: Option<&str>, body: &[u8]) -> DecodedDocument {
    // 1. HTTP header Content-Type charset。decode() 的 BOM 嗅探会覆盖所选
    //    编码（违反 HTTP > BOM），故用无 BOM 处理入口，随后剥前导 U+FEFF。
    if let Some(encoding) = content_type
        .and_then(charset_from_content_type)
        .and_then(|label| Encoding::for_label(label.as_bytes()))
    {
        let (text, _) = encoding.decode_without_bom_handling(body);
        return DecodedDocument {
            text: strip_leading_bom_char(&text),
            encoding: encoding.name(),
        };
    }
    // 2. BOM（UTF-8 / UTF-16LE / UTF-16BE）。decode() 自带 BOM 嗅探与剥离。
    if let Some(encoding) = bom_encoding(body) {
        let (text, encoding, _) = encoding.decode(body);
        return DecodedDocument {
            text: text.into_owned(),
            encoding: encoding.name(),
        };
    }
    // 3. meta charset / http-equiv 预扫（前 MAX_PRESCAN_BYTES 字节）。
    //    此时字节流已无 BOM 前缀，decode() 与无 BOM 入口等价。
    if let Some(encoding) = prescan_charset(&body[..body.len().min(MAX_PRESCAN_BYTES)]) {
        let (text, encoding, _) = encoding.decode(body);
        return DecodedDocument {
            text: text.into_owned(),
            encoding: encoding.name(),
        };
    }
    // 4. 默认 UTF-8
    let (text, _, _) = UTF_8.decode(body);
    DecodedDocument {
        text: text.into_owned(),
        encoding: UTF_8.name(),
    }
}

/// 解码后剥掉前导 U+FEFF（BOM 字节在编码一致的解码下会变成该字符）。
fn strip_leading_bom_char(text: &str) -> String {
    match text.strip_prefix('\u{FEFF}') {
        Some(rest) => rest.to_owned(),
        None => text.to_owned(),
    }
}

/// BOM 嗅探：只认 WHATWG Encoding Standard 的三种 BOM。
fn bom_encoding(body: &[u8]) -> Option<&'static Encoding> {
    if body.starts_with(&[0xEF, 0xBB, 0xBF]) {
        Some(encoding_rs::UTF_8)
    } else if body.starts_with(&[0xFF, 0xFE]) {
        Some(encoding_rs::UTF_16LE)
    } else if body.starts_with(&[0xFE, 0xFF]) {
        Some(encoding_rs::UTF_16BE)
    } else {
        None
    }
}

/// 从 `Content-Type` 头取 `charset` 参数（如 `text/html; charset=GBK`）。
/// 无参数或空值 → `None`；允许 `=` 两侧空白，引号按惯例剥掉。
fn charset_from_content_type(content_type: &str) -> Option<String> {
    for part in content_type.split(';') {
        let part = part.trim();
        let Some(name) = part.get(..7) else {
            continue;
        };
        if !name.eq_ignore_ascii_case("charset") {
            continue;
        }
        let Some(value) = part[7..].trim_start().strip_prefix('=') else {
            continue;
        };
        let value = value.trim_matches(|c: char| c.is_whitespace() || c == '"' || c == '\'');
        if !value.is_empty() {
            return Some(value.to_owned());
        }
    }
    None
}

/// 从 http-equiv meta 的 `content` 值里取编码标签（WHATWG HTML §13.2.3.5
/// 「getting an encoding」简化档）：大小写不敏感找 `charset`，跳过空白与 `=`，
/// 取引号内或至空白 / `;` 为止的标签；`charset` 后不跟 `=` 则继续找下一处。
fn charset_from_content(content: &str) -> Option<String> {
    let lower = content.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find("charset") {
        let start = from + found;
        let after = content.get(start + "charset".len()..)?;
        let Some(value) = after.trim_start().strip_prefix('=') else {
            from = start + "charset".len();
            continue;
        };
        let value = value.trim_start();
        for quote in ['"', '\''] {
            if let Some(inner) = value.strip_prefix(quote) {
                return inner
                    .find(quote)
                    .map(|end| inner[..end].to_owned())
                    .filter(|label| !label.is_empty());
            }
        }
        let end = value
            .find(|c: char| c.is_whitespace() || c == ';')
            .unwrap_or(value.len());
        return if value[..end].is_empty() {
            None
        } else {
            Some(value[..end].to_owned())
        };
    }
    None
}

/// WHATWG HTML §13.2.3.5「prescan a byte stream to determine its encoding」
/// 简化档：逐标签扫描，识别 `<meta charset=…>` 与
/// `<meta http-equiv=content-type content=…charset=…>`（属性顺序无关），
/// 跳过注释 / DOCTYPE / bogus comment；第一个能解析出**已知**编码标签的
/// `<meta>` 胜出（未知标签继续向后扫，规范同义）；标签未闭合（EOF）终止。
fn prescan_charset(head: &[u8]) -> Option<&'static Encoding> {
    let mut i = 0;
    while i < head.len() {
        if head[i] != b'<' {
            i += 1;
            continue;
        }
        let Some(&next) = head.get(i + 1) else {
            break;
        };
        match next {
            b'!' => {
                // `<!--` 注释跳到 `-->`；其他 `<!`（DOCTYPE 等）跳到 `>`
                if head.get(i + 2) == Some(&b'-') && head.get(i + 3) == Some(&b'-') {
                    let search = i + 4;
                    i = match find_subslice(&head[search.min(head.len())..], b"-->") {
                        Some(pos) => search + pos + 3,
                        None => head.len(),
                    };
                } else {
                    i = match head[i..].iter().position(|&byte| byte == b'>') {
                        Some(pos) => i + pos + 1,
                        None => head.len(),
                    };
                }
            }
            b'?' => {
                // bogus comment（`<?xml …?>`）：跳到 `>`
                i = match head[i..].iter().position(|&byte| byte == b'>') {
                    Some(pos) => i + pos + 1,
                    None => head.len(),
                };
            }
            byte if byte.is_ascii_alphabetic() => {
                let Some(tag) = scan_tag(head, i) else {
                    break; // 标签未闭合：EOF 终止预扫
                };
                if tag.name == "meta"
                    && let Some(encoding) = meta_charset(&tag.attrs)
                        .and_then(|label| Encoding::for_label(label.as_bytes()))
                {
                    return Some(encoding);
                }
                i = tag.end;
            }
            _ => i += 1,
        }
    }
    None
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// 预扫解析出的开始标签。
struct ScannedTag {
    /// 小写标签名。
    name: String,
    /// 属性表（小写属性名，保留原值）。
    attrs: Vec<(String, String)>,
    /// 标签后位置（`>` 的下一个字节）。
    end: usize,
}

/// 从 `head[start] == b'<'` 起解析一个开始标签。标签未闭合（EOF）返回 `None`。
fn scan_tag(head: &[u8], start: usize) -> Option<ScannedTag> {
    let is_separator =
        |byte: u8| matches!(byte, b' ' | b'\t' | b'\n' | b'\x0C' | b'\r' | b'/' | b'>');
    let is_space = |byte: u8| matches!(byte, b' ' | b'\t' | b'\n' | b'\x0C' | b'\r');

    let mut i = start + 1; // 跳过 '<'
    let name_start = i;
    while i < head.len() && !is_separator(head[i]) {
        i += 1;
    }
    let name = String::from_utf8_lossy(&head[name_start..i]).to_ascii_lowercase();
    let mut attrs: Vec<(String, String)> = Vec::new();
    loop {
        // 跳过空白与斜杠（自闭合 `/` 视为分隔符，简化档不单独区分）
        while i < head.len() && (is_space(head[i]) || head[i] == b'/') {
            i += 1;
        }
        let Some(&byte) = head.get(i) else {
            return None; // EOF
        };
        if byte == b'>' {
            return Some(ScannedTag {
                name,
                attrs,
                end: i + 1,
            });
        }
        // 属性名
        let attr_start = i;
        while i < head.len() && !is_separator(head[i]) && head[i] != b'=' {
            i += 1;
        }
        if i >= head.len() {
            return None; // EOF
        }
        let attr_name = String::from_utf8_lossy(&head[attr_start..i]).to_ascii_lowercase();
        while i < head.len() && is_space(head[i]) {
            i += 1;
        }
        let value = if head.get(i) == Some(&b'=') {
            i += 1;
            while i < head.len() && is_space(head[i]) {
                i += 1;
            }
            match head.get(i) {
                Some(&quote @ (b'"' | b'\'')) => {
                    i += 1;
                    let value_start = i;
                    while i < head.len() && head[i] != quote {
                        i += 1;
                    }
                    if i >= head.len() {
                        return None; // 引号未闭合：EOF 终止
                    }
                    let value = String::from_utf8_lossy(&head[value_start..i]).into_owned();
                    i += 1; // 跳过闭合引号
                    value
                }
                _ => {
                    let value_start = i;
                    while i < head.len() && !is_space(head[i]) && head[i] != b'>' {
                        i += 1;
                    }
                    String::from_utf8_lossy(&head[value_start..i]).into_owned()
                }
            }
        } else {
            String::new() // 空属性语法
        };
        attrs.push((attr_name, value));
    }
}

/// `<meta>` 的编码产出：优先 `charset` 属性；否则要求 `http-equiv=content-type`
/// 且从 `content` 属性取（WHATWG §13.2.3.5）。
fn meta_charset(attrs: &[(String, String)]) -> Option<String> {
    if let Some((_, value)) = attrs.iter().find(|(name, _)| name == "charset") {
        return Some(value.clone());
    }
    let is_content_type = attrs
        .iter()
        .any(|(name, value)| name == "http-equiv" && value.eq_ignore_ascii_case("content-type"));
    if !is_content_type {
        return None;
    }
    let content = attrs
        .iter()
        .find(|(name, _)| name == "content")
        .map(|(_, value)| value.clone())?;
    charset_from_content(&content)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GBK 编码的「中」。
    const GBK_ZHONG: &[u8] = &[0xD6, 0xD0];

    // ---- 第 1 层：HTTP Content-Type charset ----

    #[test]
    fn http_charset_gbk_decodes() {
        let doc = decode_document(Some("text/html; charset=GBK"), GBK_ZHONG);
        assert_eq!(doc.text, "中");
        assert_eq!(doc.encoding, "GBK");
    }

    #[test]
    fn http_charset_wins_over_bom() {
        // UTF-8 BOM 在前也不改变所选编码：HTTP 层优先于 BOM 层
        let mut body = vec![0xEF, 0xBB, 0xBF];
        body.extend_from_slice(GBK_ZHONG);
        let doc = decode_document(Some("text/html; charset=gbk"), &body);
        assert_eq!(doc.encoding, "GBK");
    }

    #[test]
    fn http_charset_wins_over_meta() {
        let mut body = b"<meta charset=\"utf-8\">".to_vec();
        body.extend_from_slice(GBK_ZHONG);
        let doc = decode_document(Some("text/html; charset=gbk"), &body);
        assert_eq!(doc.encoding, "GBK");
    }

    #[test]
    fn http_charset_utf8_strips_bom_bytes() {
        // BOM 作为编码选择器让位 HTTP，但 BOM 字节本身不应进入文本
        let mut body = vec![0xEF, 0xBB, 0xBF];
        body.extend_from_slice(b"x");
        let doc = decode_document(Some("text/html; charset=utf-8"), &body);
        assert_eq!(doc.text, "x");
        assert_eq!(doc.encoding, "UTF-8");
    }

    #[test]
    fn http_charset_without_param_falls_to_next_layer() {
        // 无 charset 参数 → 落到 BOM / meta 层
        let mut body = GBK_ZHONG.to_vec();
        body.extend_from_slice(b"<meta charset=gbk>");
        let doc = decode_document(Some("text/html"), &body);
        assert_eq!(doc.encoding, "GBK");
    }

    #[test]
    fn http_unknown_label_falls_to_meta() {
        let mut body = b"<meta charset=gbk>".to_vec();
        body.extend_from_slice(GBK_ZHONG);
        let doc = decode_document(Some("text/html; charset=not-a-real-label"), &body);
        assert_eq!(doc.encoding, "GBK");
    }

    // ---- 第 2 层：BOM ----

    #[test]
    fn bom_utf8_wins_over_meta() {
        let mut body = vec![0xEF, 0xBB, 0xBF];
        body.extend_from_slice(b"<meta charset=\"gbk\">\xE4\xB8\xAD");
        let doc = decode_document(None, &body);
        assert!(doc.text.contains("中"));
        assert_eq!(doc.encoding, "UTF-8");
    }

    #[test]
    fn bom_utf16le_decodes() {
        let mut body = vec![0xFF, 0xFE];
        body.extend_from_slice(&[0x2D, 0x4E]); // 中 (UTF-16LE)
        let doc = decode_document(None, &body);
        assert_eq!(doc.text, "中");
        assert_eq!(doc.encoding, "UTF-16LE");
    }

    #[test]
    fn bom_utf16be_decodes() {
        let mut body = vec![0xFE, 0xFF];
        body.extend_from_slice(&[0x4E, 0x2D]); // 中 (UTF-16BE)
        let doc = decode_document(None, &body);
        assert_eq!(doc.text, "中");
        assert_eq!(doc.encoding, "UTF-16BE");
    }

    // ---- 第 3 层：meta 预扫 ----

    #[test]
    fn meta_charset_unquoted_and_single_quoted() {
        for head in [
            &b"<meta charset=gbk>"[..],
            b"<meta charset='gbk'>",
            b"<meta charset=\"gbk\">",
            b"<META CHARSET = \"GBK\" >",
        ] {
            let mut body = head.to_vec();
            body.extend_from_slice(GBK_ZHONG);
            let doc = decode_document(None, &body);
            assert_eq!(
                doc.encoding,
                "GBK",
                "head: {}",
                String::from_utf8_lossy(head)
            );
            assert!(doc.text.ends_with("中"));
        }
    }

    #[test]
    fn meta_http_equiv_extracts_charset() {
        let mut body =
            b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=gbk\">".to_vec();
        body.extend_from_slice(GBK_ZHONG);
        let doc = decode_document(None, &body);
        assert_eq!(doc.encoding, "GBK");
        assert!(doc.text.ends_with("中"));
    }

    #[test]
    fn meta_http_equiv_attribute_order_free() {
        // content 在前、http-equiv 在后，属性值不带引号
        let mut body = b"<meta content=text/html;charset=gbk http-equiv=Content-Type>".to_vec();
        body.extend_from_slice(GBK_ZHONG);
        let doc = decode_document(None, &body);
        assert_eq!(doc.encoding, "GBK");
    }

    #[test]
    fn meta_charset_must_come_within_1024_bytes() {
        // 长注释把 meta 推出 1024 字节窗口 → 不再命中，回落默认 UTF-8
        let mut body = b"<!--".to_vec();
        body.extend(std::iter::repeat_n(b'x', 1030));
        body.extend_from_slice(b"--><meta charset=\"gbk\">");
        body.extend_from_slice(GBK_ZHONG);
        let doc = decode_document(None, &body);
        assert_eq!(doc.encoding, "UTF-8");
    }

    #[test]
    fn comment_hidden_meta_is_ignored() {
        let body = b"<!-- <meta charset=gbk> --><meta charset=\"utf-8\">\xE4\xB8\xAD".to_vec();
        let doc = decode_document(None, &body);
        assert_eq!(doc.encoding, "UTF-8");
        assert!(doc.text.contains("中"));
    }

    #[test]
    fn multiple_metas_first_known_label_wins() {
        // 第一个 meta 的标签未知：继续向后扫（规范语义），第二个命中
        let mut body = b"<meta charset=bogus-label><meta charset=gbk>".to_vec();
        body.extend_from_slice(GBK_ZHONG);
        let doc = decode_document(None, &body);
        assert_eq!(doc.encoding, "GBK");
    }

    // ---- 第 4 层：默认 UTF-8 ----

    #[test]
    fn no_hints_defaults_to_utf8() {
        let doc = decode_document(None, "中".as_bytes());
        assert_eq!(doc.text, "中");
        assert_eq!(doc.encoding, "UTF-8");
    }

    #[test]
    fn invalid_bytes_are_replaced_without_panic() {
        let doc = decode_document(None, &[0x80, b'a']);
        assert_eq!(doc.text, "\u{FFFD}a");
        assert_eq!(doc.encoding, "UTF-8");
    }

    #[test]
    fn empty_body_decodes_to_empty_text() {
        let doc = decode_document(None, b"");
        assert_eq!(doc.text, "");
        assert_eq!(doc.encoding, "UTF-8");
    }

    // ---- Content-Type 解析 ----

    #[test]
    fn charset_from_content_type_cases() {
        assert_eq!(
            charset_from_content_type("text/html; charset=GBK").as_deref(),
            Some("GBK")
        );
        assert_eq!(
            charset_from_content_type("text/html;charset=\"gb18030\"").as_deref(),
            Some("gb18030")
        );
        assert_eq!(
            charset_from_content_type("text/html; Charset = Shift_JIS ").as_deref(),
            Some("Shift_JIS")
        );
        assert_eq!(charset_from_content_type("text/html"), None);
        assert_eq!(charset_from_content_type("text/html; charset="), None);
    }

    #[test]
    fn charset_from_content_restarts_on_missing_equals() {
        // 第一处 "charset" 后不跟 =：继续找下一处
        assert_eq!(
            charset_from_content("the charset here; charset=gbk").as_deref(),
            Some("gbk")
        );
        assert_eq!(charset_from_content("no encoding here"), None);
    }
}
