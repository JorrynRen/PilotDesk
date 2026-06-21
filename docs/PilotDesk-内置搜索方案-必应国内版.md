# PilotDesk 内置搜索方案 — 必应国内版 HTML 抓取

> **状态**: 待详细讨论和开发  
> **关联**: [FunctionCalling-联网搜索技术方案-v1.0.md](./PilotDesk-FunctionCalling-联网搜索技术方案-v1.0.md) — 第2.2节搜索引擎选型补充  

---

## 1. 问题背景

v1.0 技术方案中搜索引擎选型为 SearXNG（自部署）和 Tavily（SaaS），但在实际使用场景中存在以下问题：

- **SearXNG 在线平台**：需要注册、API Key、服务不稳定
- **SearXNG Docker 部署**：用户需额外安装 Docker，软件可移植性差
- **Tavily 等商业服务**：需注册、需 API Key、免费额度有限
- **DuckDuckGo**：国内无法访问

**核心诉求**：搜索能力应**开箱即用**，用户安装软件后无需任何配置即可联网搜索，不依赖外部服务或额外安装。

---

## 2. 方案选型

### 2.1 国内可访问搜索引擎对比

| 引擎 | 国内可用 | 反爬难度 | 搜索质量 | 需注册/API | 评估 |
|------|---------|---------|---------|-----------|------|
| **必应国内版** `cn.bing.com` | 可用 | 中等（可控） | 高（中文优秀） | 否 | **首选** |
| 百度 `baidu.com` | 可用 | 高（JS渲染+验证码） | 高 | 否 | 备选，反爬严格 |
| 搜狗 `sogou.com` | 可用 | 低 | 中 | 否 | 备选，质量一般 |
| 360搜索 `so.com` | 可用 | 低 | 中 | 否 | 备选，质量一般 |

### 2.2 推荐：必应国内版 HTML 抓取

**理由**：

1. **国内可直接访问** — `cn.bing.com` 无需翻墙
2. **搜索质量高** — 中文搜索质量优秀，时效性强
3. **零配置零注册** — 直接 HTTP GET 请求，无需 API Key 或账号
4. **HTML 结构清晰** — 搜索结果在 `li.b_algo` 元素中，标题 `h2 > a`，摘要 `.b_caption p`，解析简单稳定
5. **Rust 生态成熟** — `reqwest`（已有）+ `scraper`（CSS 选择器）即可实现
6. **体积影响极小** — 仅增加 `scraper` 一个 crate 依赖（约 50KB）

---

## 3. 技术实现概要

### 3.1 架构位置

搜索逻辑放在 **Rust 后端（Tauri command）** 而非前端 TypeScript：

```
用户提问 → 模型触发 web_search 工具
  → 前端调用 Rust Tauri command "web_search" { query }
    → reqwest GET https://cn.bing.com/search?q={query}&count=10
    → scraper crate 解析 HTML → 提取标题/链接/摘要
    → 返回 Vec<{ title, url, snippet }>
  → 工具结果回传模型 → 模型整合回答用户
```

**优势**：Rust 的 HTTP 客户端性能好、可自定义 User-Agent 模拟浏览器、反爬能力更强、不增加前端 bundle 大小。

### 3.2 Rust 实现要素

- **HTTP 请求**：`reqwest`（已在 Cargo.toml 中）
- **HTML 解析**：`scraper` crate（新增依赖，~50KB）
- **目标 URL**：`https://cn.bing.com/search?q={query}&count=10`
- **CSS 选择器**：
  - 结果容器：`li.b_algo`
  - 标题：`h2 a[href]`
  - 链接：`h2 a` 的 `href` 属性
  - 摘要：`.b_caption p` 或 `.b_caption .b_attribution`
- **反爬策略**：自定义 User-Agent、正常频率请求（用户级别，非批量爬虫）

### 3.3 Rust 代码结构（示意）

```rust
#[tauri::command]
pub async fn web_search(query: String) -> Result<Vec<SearchResult>, AppError> {
    let url = format!(
        "https://cn.bing.com/search?q={}&count=10",
        urlencoding::encode(&query)
    );
    
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 ...")
        .build()?;
    
    let html = client.get(&url).send().await?.text().await?;
    
    let document = scraper::Html::parse_document(&html);
    let selector = scraper::Selector::parse("li.b_algo").unwrap();
    
    let mut results = Vec::new();
    for element in document.select(&selector).take(10) {
        let title = element.select(&title_sel).next()
            .map(|e| e.text().collect::<String>()).unwrap_or_default();
        let url = element.select(&link_sel).next()
            .and_then(|e| e.value().attr("href")).unwrap_or_default();
        let snippet = element.select(&snippet_sel).next()
            .map(|e| e.text().collect::<String>()).unwrap_or_default();
        
        if !title.is_empty() {
            results.push(SearchResult { title, url, snippet });
        }
    }
    
    Ok(results)
}
```

### 3.4 设置页策略

| 选项 | 说明 |
|------|------|
| **必应（内置）** | 默认引擎，开箱即用，无需任何配置 |
| SearXNG | 高级选项，自部署用户可选 |
| Tavily | 高级选项，有 API Key 的用户可选 |

默认显示"必应（内置）"，用户无需感知搜索引擎配置；展开高级设置可切换引擎。

---

## 4. 与 v1.0 方案的变更对照

| 项目 | v1.0 方案 | 更新后 |
|------|----------|--------|
| 默认搜索引擎 | SearXNG | **必应国内版（内置）** |
| 首选/备选 | 首选 SearXNG，备选 Tavily | 首选 **必应内置**，备选 SearXNG / Tavily |
| 实现位置 | 前端 TypeScript（searchProvider.ts） | **Rust 后端（Tauri command）** |
| 新增依赖 | 无 | Rust crate: `scraper`（~50KB） |
| 零配置可用 | 否（需部署 SearXNG 或配置 Tavily Key） | **是（默认必应开箱即用）** |
| 需要注册/Key | 视引擎而定 | **不需要（默认引擎）** |
| Docker 依赖 | SearXNG 需要 | **不需要（默认引擎）** |

### 不变的部分

- Function Calling 协议方案不变（标准 OpenAI tools 协议）
- 工具循环架构不变（前端 while 循环）
- 推理可视化方案不变
- 数据库改动不变
- toolExecutor 可扩展设计不变（内置引擎替换为必应即可）

---

## 5. 待讨论事项

- [ ] 必应 HTML 结构在不同地区/语言下是否一致（繁体、英文切换）
- [ ] 搜索结果中是否需要过滤广告链接（Bing 搜索页面顶部有广告位）
- [ ] 反爬限制的具体阈值（多少次/分钟会触发验证码）
- [ ] 是否需要百度作为 fallback 引擎（必应失败时自动切换）
- [ ] 搜索结果缓存策略（相同 query 短时间内是否缓存）
- [ ] 隐私合规：搜索请求日志是否存储、如何告知用户
