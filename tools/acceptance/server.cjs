// Nexty 实机验收站（仅回环，端口 8472）。
//
// 用途：为 Computer Use 实机验收提供可复现的页面与端点。浏览器本身没有
// 无头模式，验收靠架构师点窗口；本服务器负责把「判据」变成固定 URL，
// 并用 /stats 记录每个 URL 的请求次数，使「缓存命中 / 字体去重 / 302 落点」
// 这类不可见行为可以被计数断言。
//
// 运行：node tools/acceptance/server.cjs      （PORT 环境变量可覆盖端口）
// 索引：http://127.0.0.1:8472/
//
// 约定：
// - pages/ 下除 batch1 的动态别名外，一律按 URL 路径静态出文件
//   （/core/ → pages/core/index.html，/legacy/t3/links.html → 同名文件）。
// - 需要响应头（302、Cache-Control、charset=gbk）或跨目录取字节（字体）的
//   路由在下方显式登记，避免把「验收前提」藏进页面 HTML。

const http = require("node:http");
const { readFileSync, existsSync, statSync } = require("node:fs");
const { join, extname } = require("node:path");

const PAGES = join(__dirname, "pages");
const REPO = join(__dirname, "..", "..");
const PORT = Number(process.env.PORT || 8472);

// 显式路由：URL → pages/ 下文件。带特殊响应头的走独立分支。
const ALIASES = {
  "/k2": "batch1/k2/k2-visual.html",
  "/k2z": "batch1/k2/k2-zoom.html",
  "/k2u": "batch1/k2/k2-decoration.html",
  "/k2one": "batch1/k2/k2-link-single.html",
  "/k2a": "batch1/k2/k2-link-author.html",
  "/font": "batch1/k4/font-face.html",
  "/font404": "batch1/k4/font-missing.html",
  "/landing/dir/page.html": "batch1/k3/landing-page.html",
  "/landing/dir/sub.html": "batch1/k3/landing-sub.html",
  "/landing/dir/dot.png": "core/dot.png",
};

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".png": "image/png",
  ".ttf": "font/ttf",
  ".hex": "text/plain; charset=ascii",
};

const counts = new Map();
const bump = (key) => counts.set(key, (counts.get(key) || 0) + 1);

function send(res, status, headers, body) {
  res.writeHead(status, headers);
  res.end(body);
}

function sendFile(res, file, extraHeaders = {}) {
  if (!existsSync(file) || !statSync(file).isFile()) {
    send(res, 404, { "Content-Type": "text/plain; charset=utf-8" }, "not found");
    return;
  }
  send(
    res,
    200,
    { "Content-Type": MIME[extname(file)] ?? "application/octet-stream", ...extraHeaders },
    readFileSync(file)
  );
}

function resolveStatic(url) {
  // URL 路径直接映射到 pages/；目录补 index.html
  const clean = join(PAGES, url);
  if (!clean.startsWith(PAGES)) return null; // 防目录穿越
  if (existsSync(clean) && statSync(clean).isDirectory()) return join(clean, "index.html");
  return clean;
}

http
  .createServer((req, res) => {
    const url = req.url.split("?")[0].split("#")[0];
    const decoded = decodeURIComponent(url);
    console.log(`REQ ${req.method} ${decoded} ua=${req.headers["user-agent"] ?? "-"}`);
    bump(decoded);

    if (decoded === "/stats") {
      send(res, 200, { "Content-Type": "application/json; charset=utf-8" }, JSON.stringify(Object.fromEntries(counts), null, 0));
      return;
    }
    if (decoded === "/reset") {
      counts.clear();
      send(res, 200, { "Content-Type": "text/plain; charset=utf-8" }, "reset");
      return;
    }
    if (decoded === "/gbk") {
      // GBK 页面字节：由 gbk-page.src.html 经 iconv 预生成（见 README）
      const hex = readFileSync(join(PAGES, "batch1/k3/gbk-page.hex"), "utf8").trim();
      send(res, 200, { "Content-Type": "text/html; charset=gbk" }, Buffer.from(hex, "hex"));
      return;
    }
    if (decoded === "/redir") {
      send(res, 302, { Location: "/landing/dir/page.html" }, "");
      return;
    }
    if (decoded === "/cached") {
      send(
        res,
        200,
        { "Content-Type": "text/html; charset=utf-8", "Cache-Control": "max-age=120" },
        readFileSync(join(PAGES, "batch1/k3/cached.html"))
      );
      return;
    }
    if (decoded === "/font/space-mono.ttf") {
      // 测试字体来自 text 层已入库的 OFL 字体，不在本目录复制一份
      sendFile(res, join(REPO, "crates/nexty-text/tests/data/SpaceMono-Regular.ttf"));
      return;
    }

    const alias = ALIASES[decoded];
    if (alias) {
      sendFile(res, join(PAGES, alias));
      return;
    }

    const file = resolveStatic(decoded === "/" ? "/index.html" : decoded);
    if (file) {
      sendFile(res, file);
      return;
    }
    send(res, 404, { "Content-Type": "text/plain; charset=utf-8" }, "not found");
  })
  .listen(PORT, "127.0.0.1", () => {
    console.log(`Nexty acceptance server: http://127.0.0.1:${PORT}`);
  });
