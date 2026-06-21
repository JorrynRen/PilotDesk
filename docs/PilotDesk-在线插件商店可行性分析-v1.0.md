# PilotDesk 在线插件商店可行性分析 v1.0

> 分析日期：2026-06-21
> 状态：可行性分析（不涉及代码调整）
> 核心方案：基于 GitHub 仓库的文件夹式插件资源库 + CI 自动索引

---

## 1. 需求定义

### 1.1 核心流程

```
插件开发者
  +-- 将插件目录（含 manifest.json + 代码文件）提交到 GitHub 仓库
  +-- CI 自动扫描插件目录，生成 index.json 索引
  +-- 仓库结构天然可见，无需手动维护索引

PilotDesk 用户
  +-- 在应用内浏览在线插件列表（名称、描述、版本、作者等）
  +-- 点击"安装"按钮 → 从 GitHub 下载插件目录 → 解压到本地插件目录
  +-- 安装完成后自动发现并加载
```

### 1.2 功能范围

| 功能 | 说明 | 优先级 |
|------|------|--------|
| 插件浏览 | 展示插件名称、描述、版本、作者、图标 | P0 |
| 一键安装 | 从 GitHub 下载插件目录并安装到本地 | P0 |
| 版本更新 | 检测本地插件版本 vs 在线版本，提示更新 | P1 |
| 一键更新 | 下载新版本替换旧版本 | P1 |
| 插件搜索 | 按名称/描述搜索在线插件 | P1 |
| 分类/标签 | 按功能分类浏览 | P2 |
| 自动更新 | 后台检查更新并静默升级 | P3 |

---

## 2. 方案设计

### 2.1 架构总览

```
+----------------------------------------------------------------------+
|           GitHub: JorrynRen/PilotDesk                                 |
|                                                                       |
|  server/market/                                                       |
|  +-- plugins/                   # 插件目录（含索引）                  |
|  |   +-- index.json             # CI 自动生成（勿手动编辑）            |
|  |   +-- hello-world/           # 插件目录名 = 插件 ID                |
|  |   |   +-- manifest.json                                          |
|  |   |   +-- index.js                                                |
|  |   |   +-- icon.png                                                |
|  |   +-- news-collector/                                             |
|  |   |   +-- manifest.json                                          |
|  |   |   +-- index.js                                                |
|  |   +-- ...                                                         |
|  +-- .github/workflows/                                              |
|      +-- generate-index.yml     # CI: 自动扫描生成 index.json        |
+----------------------------------------------------------------------+
        |
        |--- raw: https://raw.githubusercontent.com/JorrynRen/PilotDesk/main/server/market/plugins/index.json
        |       实时读取，无缓存，适合调试和强制刷新
        |
        |--- CDN: https://cdn.jsdelivr.net/gh/JorrynRen/PilotDesk@main/server/market/plugins/index.json
        |       全球 CDN 加速，低延迟
        |
        v
+----------------------------------------------------------------------+
|                        PilotDesk 客户端                                |
|                                                                       |
|  读取策略：raw + CDN 双模式                                           |
|  1. 优先请求 CDN（速度快）                                            |
|  2. 若 CDN 数据过期或失败，回退到 raw（实时）                         |
|  3. 用户可手动"强制刷新"→ 直接走 raw                                 |
|                                                                       |
|  +---------------------+  +---------------------------------------+  |
|  | OnlinePluginStore    |  | PluginHost (本地)                     |  |
|  | - fetchIndex(raw)    |  | - 扫描 plugins_dir                   |  |
|  | - fetchIndex(cdn)    |  | - 安装/卸载                          |  |
|  | - 插件列表展示        |  | - 启用/禁用                          |  |
|  | - 一键安装            |  |                                     |  |
|  | - 版本对比            |  |                                     |  |
|  +----------+----------+  +-------------------+-------------------+  |
|             |                                  |                      |
|  +----------v----------+  +-------------------v-------------------+  |
|  | pluginStore (Zustand)|  | PluginManager (现有 UI)              |  |
|  | - onlinePlugins[]    |  | - 本地/在线标签切换                  |  |
|  | - localPlugins[]     |  | - 在线商店面板                       |  |
|  +---------------------+  +---------------------------------------+  |
+----------------------------------------------------------------------+
```

### 2.2 仓库结构规范

```
JorrynRen/PilotDesk/
├── server/
│   └── market/
│       └── plugins/                # 插件目录（含索引）
│           ├── index.json          # CI 自动生成，勿手动编辑
│           ├── hello-world/        # 插件目录名 = 插件 ID
│           │   ├── manifest.json   # 插件清单（必填）
│           │   ├── index.js        # 插件入口（必填）
│           │   ├── icon.png        # 插件图标（可选）
│           │   └── README.md       # 插件说明（可选）
│           ├── news-collector/
│           │   ├── manifest.json
│           │   ├── index.js
│           │   └── icon.png
│           └── ...
├── .github/workflows/
│   └── generate-index.yml          # CI 工作流
└── ...
```

**核心原则**：`server/market/plugins/` 下每个子目录即一个插件，目录名 = 插件 ID。`index.json` 放在 `plugins/` 目录内，与插件目录平级，不污染外层其他板块。开发者只需将插件目录放入 `plugins/` 并提交，CI 自动完成索引生成。

### 2.3 自动索引生成（GitHub Actions）

**工作流文件** `.github/workflows/generate-index.yml`：

```yaml
name: Generate Plugin Index
on:
  push:
    branches: [main]
    paths:
      - 'server/market/plugins/**'   # 仅插件目录变化时触发
  workflow_dispatch:                 # 支持手动触发

permissions:
  contents: write                    # 允许提交 index.json 回仓库

jobs:
  generate:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Generate index.json
        run: |
          echo '{
            "schemaVersion": "1.0",
            "updatedAt": "'$(date -u +"%Y-%m-%dT%H:%M:%SZ")'",
            "plugins": [' > server/market/plugins/index.json
          first=true
          for dir in server/market/plugins/*/; do
            if [ -f "${dir}manifest.json" ]; then
              if [ "$first" = true ]; then
                first=false
              else
                echo ',' >> server/market/plugins/index.json
              fi
              plugin_id=$(basename "$dir")
              size=$(du -sh "$dir" | cut -f1)
              manifest=$(cat "${dir}manifest.json")
              echo "$manifest" | jq --arg id "$plugin_id" \
                --arg path "plugins/$plugin_id" \
                --arg downloadUrl "https://api.github.com/repos/JorrynRen/PilotDesk/zipball/main?path=server/market/plugins/$plugin_id" \
                --arg icon "https://raw.githubusercontent.com/JorrynRen/PilotDesk/main/server/market/plugins/$plugin_id/icon.png" \
                --arg size "$size" \
                '. + {path: $path, downloadUrl: $downloadUrl, icon: $icon, size: $size}' >> server/market/plugins/index.json
            fi
          done
          echo ']}' >> server/market/plugins/index.json

      - name: Commit index.json
        run: |
          git config user.name "pilotdesk-bot"
          git config user.email "bot@pilotdesk.app"
          git add server/market/plugins/index.json
          git diff --quiet && git diff --staged --quiet || \
            git commit -m "chore: auto-generate plugin index [skip ci]"
          git push
```

**工作流说明**：

| 步骤 | 说明 |
|------|------|
| 触发条件 | 仅 `server/market/plugins/**` 有变更时触发，避免不必要的运行 |
| 扫描逻辑 | 遍历 `server/market/plugins/*/` 下所有子目录，检查是否存在 `manifest.json` |
| 数据注入 | 读取每个插件的 `manifest.json`，注入 `path`、`downloadUrl`、`icon`、`size` 字段 |
| 自动提交 | 生成新的 `server/market/plugins/index.json` 并自动 commit/push，commit message 带 `[skip ci]` 防止递归触发 |

**开发者发布插件的完整流程**：

```
1. 在本地创建插件目录（含 manifest.json + index.js）
2. 将目录复制到仓库的 server/market/plugins/ 下
3. 提交 PR → 合并到 main
4. GitHub Actions 自动扫描 → 更新 server/market/plugins/index.json
5. PilotDesk 用户刷新即可看到新插件
```

### 2.4 index.json 格式

由 CI 自动生成，前端和 Rust 侧只读消费：

```json
{
  "schemaVersion": "1.0",
  "updatedAt": "2026-06-21T00:00:00Z",
  "plugins": [
    {
      "id": "hello-world",
      "name": "Hello World",
      "version": "1.0.0",
      "description": "示例插件，展示基础面板能力",
      "author": "PilotDesk Team",
      "minAppVersion": "0.1.0",
      "permissions": ["ui:panel", "ui:toast"],
      "entry": { "main": "index.js" },
      "contributes": {
        "panels": [
          { "id": "hello-panel", "title": "Hello World", "icon": "icon.png" }
        ]
      },
      "path": "plugins/hello-world",
      "downloadUrl": "https://api.github.com/repos/JorrynRen/PilotDesk/zipball/main?path=server/market/plugins/hello-world",
      "icon": "https://raw.githubusercontent.com/JorrynRen/PilotDesk/main/server/market/plugins/hello-world/icon.png",
      "size": "12KB",
      "tags": ["demo", "ui"],
      "createdAt": "2026-06-01T00:00:00Z",
      "updatedAt": "2026-06-21T00:00:00Z"
    }
  ]
}
```

**字段说明**：

| 字段 | 来源 | 说明 |
|------|------|------|
| `id` | manifest.json | 插件唯一标识 |
| `name`/`version`/`description`/`author`/`minAppVersion`/`permissions`/`entry`/`contributes` | manifest.json | 直接从插件清单读取 |
| `path` | CI 注入 | 插件在仓库中的相对路径 |
| `downloadUrl` | CI 注入 | GitHub API zipball 下载地址 |
| `icon` | CI 注入 | 拼接 raw.githubusercontent.com 地址 |
| `size` | CI 注入 | 插件目录大小 |
| `tags` | CI 注入 | 可从 manifest.json 扩展字段读取 |
| `createdAt`/`updatedAt` | CI 注入 | 基于当前时间生成 |

### 2.5 索引读取策略：raw + CDN 双模式

| 模式 | URL | 特点 | 使用场景 |
|------|-----|------|---------|
| **raw** | `https://raw.githubusercontent.com/JorrynRen/PilotDesk/main/server/market/plugins/index.json` | 实时读取，无缓存，直连 GitHub | 强制刷新、CDN 降级 |
| **CDN** | `https://cdn.jsdelivr.net/gh/JorrynRen/PilotDesk@main/server/market/plugins/index.json` | 全球 CDN 加速，低延迟 | 默认读取 |

**客户端读取策略**：

```
fetchIndex(forceRefresh = false):
  1. if forceRefresh == true:
     → 直接走 raw 模式（用户手动点击"刷新"时）
     → 返回结果

  2. 优先请求 CDN:
     → GET https://cdn.jsdelivr.net/gh/JorrynRen/PilotDesk@main/server/market/index.json
     → 成功 → 返回结果
     → 失败（网络错误/超时/解析错误）→ 走 raw 降级

  3. raw 降级:
     → GET https://raw.githubusercontent.com/JorrynRen/PilotDesk/main/server/market/index.json
     → 成功 → 返回结果
     → 失败 → 提示"商店暂不可用"
```

**jsDelivr CDN 强制刷新说明**：

jsDelivr 默认缓存 TTL 较长，但可通过以下方式确保客户端拿到最新数据：

| 方式 | 说明 |
|------|------|
| 用户手动刷新 | 客户端直接走 raw 模式，绕过 CDN |
| 自动降级 | CDN 返回过期数据时（如 status 304），自动回退 raw |
| URL 版本号 | 后续可考虑在 index.json 中加 `updatedAt` 字段，客户端据此判断是否过期 |

### 2.6 下载机制

使用 GitHub API 的 zipball 端点下载仓库子目录：

```
GET https://api.github.com/repos/JorrynRen/PilotDesk/zipball/main?path=server/market/plugins/hello-world
```

**返回内容**：一个 zip 压缩包，包含 `server/market/plugins/hello-world/` 目录及其所有文件。

**客户端处理流程**：

```
1. Rust 侧发起 HTTP GET 请求到 downloadUrl
2. 收到 zip 二进制数据，保存到临时文件
3. 调用现有 PluginHost.install_from_zip() 解压到 plugins_dir
4. 清理临时文件
5. 调用 discover() 自动发现新安装的插件
```

**关于 GitHub API 速率限制**：

| 限制类型 | 未认证 | 已认证（Token） |
|---------|--------|----------------|
| 每小时请求数 | 60 | 5,000 |
| 每次安装消耗 | 1 次请求（index.json 走 CDN） | 1 次请求 |

**缓解措施**：
- `index.json` 优先走 CDN，不消耗 API 配额
- raw 降级仅在 CDN 失败时触发，频率极低
- 仅 `plugin_store_install` 的 zipball 下载消耗 API 配额
- 每个用户每小时安装 60 个插件才触达未认证上限，实际场景远低于此
- 后续可配置 GitHub Token 提升至 5,000/h

---

## 3. 技术实现分析

### 3.1 需要新增/修改的组件

#### 3.1.1 Rust 侧：插件在线商店命令（新增）

**新增文件**：`src-tauri/src/plugin/store.rs`

```rust
/// 在线插件信息（来自 index.json）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnlinePluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub min_app_version: String,
    pub permissions: Vec<String>,
    pub entry: PluginEntry,
    pub contributes: Option<PluginContributes>,
    pub path: String,
    pub download_url: String,
    pub icon: Option<String>,
    pub size: Option<String>,
    pub tags: Option<Vec<String>>,
    pub created_at: String,
    pub updated_at: String,
}

/// 插件索引
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginIndex {
    pub schema_version: String,
    pub updated_at: String,
    pub plugins: Vec<OnlinePluginInfo>,
}

/// 本地插件与在线版本对比结果
#[derive(Debug, Clone, Serialize)]
pub struct PluginVersionCompare {
    pub id: String,
    pub local_version: Option<String>,
    pub online_version: String,
    pub has_update: bool,
    pub is_installed: bool,
}
```

**新增 Tauri 命令**：

```rust
// 1. 获取在线插件列表（双模式：CDN 优先，raw 降级）
#[tauri::command]
pub async fn plugin_store_fetch_index(force_refresh: Option<bool>) -> Result<PluginIndex, String>;

// 2. 下载并安装在线插件（GitHub API zipball → 解压 → 安装）
#[tauri::command]
pub async fn plugin_store_install(plugin_id: String) -> Result<PluginInstance, String>;

// 3. 对比本地与在线版本
#[tauri::command]
pub async fn plugin_store_check_updates() -> Result<Vec<PluginVersionCompare>, String>;

// 4. 更新插件到最新版本
#[tauri::command]
pub async fn plugin_store_update(plugin_id: String) -> Result<PluginInstance, String>;
```

**实现要点**：

```rust
/// CDN 地址
const INDEX_CDN_URL: &str = "https://cdn.jsdelivr.net/gh/JorrynRen/PilotDesk@main/server/market/plugins/index.json";
/// raw 地址（降级用）
const INDEX_RAW_URL: &str = "https://raw.githubusercontent.com/JorrynRen/PilotDesk/main/server/market/plugins/index.json";

/// 获取在线插件列表（双模式读取）
#[tauri::command]
pub async fn plugin_store_fetch_index(force_refresh: Option<bool>) -> Result<PluginIndex, String> {
    let force = force_refresh.unwrap_or(false);

    if force {
        // 强制刷新 → 直接走 raw（实时）
        return fetch_index_from(&INDEX_RAW_URL).await;
    }

    // 优先 CDN
    match fetch_index_from(&INDEX_CDN_URL).await {
        Ok(index) => Ok(index),
        Err(_) => {
            // CDN 失败 → raw 降级
            log::warn!("[PluginStore] CDN 读取失败，降级到 raw");
            fetch_index_from(&INDEX_RAW_URL).await
        }
    }
}

/// 从指定 URL 获取并解析 index.json
async fn fetch_index_from(url: &str) -> Result<PluginIndex, String> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("获取插件列表失败: {}", e))?;
    let index: PluginIndex = response.json()
        .await
        .map_err(|e| format!("解析插件列表失败: {}", e))?;
    Ok(index)
}

/// 下载并安装在线插件
#[tauri::command]
pub async fn plugin_store_install(plugin_id: String) -> Result<PluginInstance, String> {
    // 1. 获取 index，找到对应插件的 download_url
    let index = plugin_store_fetch_index(Some(true)).await?;  // 安装时走 raw 确保最新
    let plugin_info = index.plugins.iter()
        .find(|p| p.id == plugin_id)
        .ok_or_else(|| format!("插件 '{}' 未找到", plugin_id))?;

    // 2. 下载 zipball 到临时目录
    let temp_dir = std::env::temp_dir().join(format!("pilotdesk-plugin-{}", plugin_id));
    let zip_path = temp_dir.join(format!("{}.zip", plugin_id));
    std::fs::create_dir_all(&temp_dir)
        .map_err(|e| format!("创建临时目录失败: {}", e))?;

    let response = reqwest::get(&plugin_info.download_url)
        .await
        .map_err(|e| format!("下载插件失败: {}", e))?;
    let bytes = response.bytes()
        .await
        .map_err(|e| format!("读取下载内容失败: {}", e))?;
    std::fs::write(&zip_path, &bytes)
        .map_err(|e| format!("保存文件失败: {}", e))?;

    // 3. 使用现有的 install_from_zip 逻辑
    let plugin_host = /* 获取 PluginHost 实例 */;
    let instance = plugin_host.install_from_zip(zip_path.to_str().unwrap())?;

    // 4. 清理临时文件
    let _ = std::fs::remove_dir_all(&temp_dir);

    Ok(instance)
}
```

**依赖新增**：

```toml
# Cargo.toml
[dependencies]
reqwest = { version = "0.12", features = ["json"] }
```

> **注意**：检查项目中是否已引入 `reqwest`。如果未引入，需新增依赖。也可考虑使用 `ureq`（更轻量）作为替代。

#### 3.1.2 前端：在线商店面板（新增）

**新增文件**：`src/components/plugin/OnlinePluginStore.tsx`

```tsx
import { useEffect, useState, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { usePluginStore } from '../../stores/pluginStore';
import type { OnlinePluginInfo } from '../../types/plugin';

export function OnlinePluginStore() {
  const { plugins, discover } = usePluginStore();
  const [onlinePlugins, setOnlinePlugins] = useState<OnlinePluginInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [installing, setInstalling] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState('');

  useEffect(() => {
    fetchOnlinePlugins(false);  // 首次加载走 CDN
  }, []);

  const fetchOnlinePlugins = async (forceRefresh: boolean) => {
    setLoading(true);
    setError(null);
    try {
      const index = await invoke<{ plugins: OnlinePluginInfo[] }>(
        'plugin_store_fetch_index',
        { forceRefresh }
      );
      setOnlinePlugins(index.plugins);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  };

  const handleInstall = async (pluginId: string) => {
    setInstalling(pluginId);
    setError(null);
    try {
      await invoke('plugin_store_install', { pluginId });
      await discover();
    } catch (err) {
      setError(String(err));
    } finally {
      setInstalling(null);
    }
  };

  // 构建本地插件版本映射
  const localVersions = useMemo(() => {
    const map = new Map<string, string>();
    for (const p of plugins) {
      map.set(p.manifest.id, p.manifest.version);
    }
    return map;
  }, [plugins]);

  // 搜索过滤
  const filteredPlugins = useMemo(() => {
    if (!searchQuery.trim()) return onlinePlugins;
    const q = searchQuery.toLowerCase();
    return onlinePlugins.filter(p =>
      p.name.toLowerCase().includes(q) ||
      p.description.toLowerCase().includes(q) ||
      p.author.toLowerCase().includes(q) ||
      p.tags?.some(t => t.toLowerCase().includes(q))
    );
  }, [onlinePlugins, searchQuery]);

  return (
    <div className="p-4">
      {/* 顶部：搜索栏 + 刷新按钮 */}
      <div className="flex items-center gap-2 mb-4">
        <div className="flex-1 relative">
          <input
            type="text"
            placeholder="搜索在线插件..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            className="search-input w-full"
          />
        </div>
        <button
          onClick={() => fetchOnlinePlugins(true)}
          className="pd-btn text-[10px] px-2 py-1 rounded shrink-0"
          style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
          title="强制刷新（直连 GitHub 获取最新数据）"
        >
          刷新
        </button>
      </div>

      {error && (
        <div className="text-xs py-2 px-3 rounded mb-3" style={{ backgroundColor: 'rgba(239,68,68,0.1)', color: '#EF4444' }}>
          {error}
        </div>
      )}

      {loading && (
        <div className="text-xs py-4 text-center" style={{ color: 'var(--text-tertiary)' }}>
          加载插件列表...
        </div>
      )}

      {!loading && filteredPlugins.length === 0 && (
        <div className="text-xs py-8 text-center" style={{ color: 'var(--text-tertiary)' }}>
          {searchQuery ? '没有匹配的插件' : '暂无可用插件'}
        </div>
      )}

      <div className="space-y-2">
        {filteredPlugins.map((plugin) => {
          const localVersion = localVersions.get(plugin.id);
          const isInstalled = !!localVersion;
          const hasUpdate = isInstalled && localVersion !== plugin.version;

          return (
            <div
              key={plugin.id}
              className="px-3 py-2.5 rounded-lg transition-colors"
              style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
            >
              {/* 标题行 */}
              <div className="flex items-center justify-between gap-2">
                <div className="flex items-center gap-2 min-w-0 flex-1">
                  {plugin.icon && (
                    <img src={plugin.icon} alt="" className="w-5 h-5 rounded shrink-0" />
                  )}
                  <span className="text-xs truncate" style={{ color: 'var(--text-primary)' }}>
                    {plugin.name}
                  </span>
                  <span className="text-[10px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>
                    v{plugin.version}
                  </span>
                  {isInstalled && (
                    <span
                      className="text-[9px] px-1 py-0.5 rounded shrink-0"
                      style={{
                        backgroundColor: hasUpdate ? 'rgba(245,158,11,0.15)' : 'rgba(16,185,129,0.15)',
                        color: hasUpdate ? '#F59E0B' : '#10B981',
                      }}
                    >
                      {hasUpdate ? '可更新' : '已安装'}
                    </span>
                  )}
                </div>
                <button
                  onClick={() => handleInstall(plugin.id)}
                  disabled={installing === plugin.id || (isInstalled && !hasUpdate)}
                  className="px-2.5 py-1 rounded text-[10px] transition-all shrink-0"
                  style={{
                    backgroundColor: installing === plugin.id
                      ? 'var(--bg-tertiary)'
                      : hasUpdate
                        ? '#F59E0B'
                        : 'var(--accent)',
                    color: installing === plugin.id ? 'var(--text-tertiary)' : '#fff',
                    opacity: isInstalled && !hasUpdate ? 0.5 : 1,
                  }}
                >
                  {installing === plugin.id ? '安装中...' : hasUpdate ? '更新' : isInstalled ? '已安装' : '安装'}
                </button>
              </div>

              {/* 描述 */}
              <p className="text-[10px] mt-1.5 line-clamp-2" style={{ color: 'var(--text-secondary)' }}>
                {plugin.description}
              </p>

              {/* 元信息 */}
              <div className="flex items-center gap-1 mt-1.5 flex-wrap">
                <span className="text-[9px] px-1.5 py-0.5 rounded" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}>
                  {plugin.author}
                </span>
                {plugin.tags?.map(tag => (
                  <span key={tag} className="text-[9px] px-1.5 py-0.5 rounded" style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}>
                    {tag}
                  </span>
                ))}
                {plugin.size && (
                  <span className="text-[9px]" style={{ color: 'var(--text-tertiary)' }}>
                    {plugin.size}
                  </span>
                )}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
```

#### 3.1.3 前端：插件商店入口（修改）

**修改文件**：`src/components/plugin/PluginManager.tsx`

在现有 PluginManager 中添加"在线商店"标签切换：

```tsx
import { OnlinePluginStore } from './OnlinePluginStore';

export function PluginManager() {
  const [activeTab, setActiveTab] = useState<'local' | 'store'>('local');

  return (
    <div className="p-4">
      {/* 标签切换 */}
      <div className="flex items-center gap-2 mb-4">
        <button
          onClick={() => setActiveTab('local')}
          className="text-xs px-3 py-1.5 rounded transition-all"
          style={{
            backgroundColor: activeTab === 'local' ? 'var(--accent)' : 'var(--bg-tertiary)',
            color: activeTab === 'local' ? '#fff' : 'var(--text-secondary)',
          }}
        >
          本地插件
        </button>
        <button
          onClick={() => setActiveTab('store')}
          className="text-xs px-3 py-1.5 rounded transition-all"
          style={{
            backgroundColor: activeTab === 'store' ? 'var(--accent)' : 'var(--bg-tertiary)',
            color: activeTab === 'store' ? '#fff' : 'var(--text-secondary)',
          }}
        >
          在线商店
        </button>
      </div>

      {activeTab === 'local' ? (
        <LocalPluginManager />  // 现有内容
      ) : (
        <OnlinePluginStore />
      )}
    </div>
  );
}
```

#### 3.1.4 类型定义扩展（修改）

**修改文件**：`src/types/plugin.ts`

新增在线插件相关类型：

```typescript
/** 在线插件信息（来自 index.json） */
export interface OnlinePluginInfo {
  id: string;
  name: string;
  version: string;
  description: string;
  author: string;
  minAppVersion: string;
  permissions: string[];
  entry: { main: string };
  contributes?: PluginContributes;
  path: string;
  downloadUrl: string;
  icon?: string;
  size?: string;
  tags?: string[];
  createdAt: string;
  updatedAt: string;
}

/** 插件索引 */
export interface PluginIndex {
  schemaVersion: string;
  updatedAt: string;
  plugins: OnlinePluginInfo[];
}

/** 版本对比结果 */
export interface PluginVersionCompare {
  id: string;
  localVersion: string | null;
  onlineVersion: string;
  hasUpdate: boolean;
  isInstalled: boolean;
}
```

### 3.2 数据流

```
用户打开插件管理 → 切换到"在线商店"
  → OnlinePluginStore 挂载
    → invoke('plugin_store_fetch_index', { forceRefresh: false })
      → Rust: 优先 GET CDN (jsDelivr)
        → 成功 → 返回 PluginIndex
        → 失败 → 降级 GET raw (raw.githubusercontent.com)
          → 成功 → 返回 PluginIndex
          → 失败 → 返回错误
    → 前端读取 usePluginStore 中的本地插件列表
    → 对比版本，标记"已安装/可更新/未安装"

用户点击"刷新"
  → fetchOnlinePlugins(true)
    → invoke('plugin_store_fetch_index', { forceRefresh: true })
      → Rust: 直接 GET raw (raw.githubusercontent.com)
      → 返回最新 PluginIndex

用户点击"安装"
  → invoke('plugin_store_install', { pluginId: 'hello-world' })
    → Rust: 获取 index（forceRefresh=true 确保最新）
    → Rust: HTTP GET zipball
    → Rust: 保存 zip 到临时目录
    → Rust: 调用现有 PluginHost.install_from_zip()
    → Rust: 清理临时文件
    → 返回 PluginInstance
  → 前端调用 discover() 刷新本地插件列表
  → 按钮状态更新为"已安装"
```

### 3.3 版本更新检测

```
本地插件列表 vs 在线插件列表对比：

for each 在线插件:
  if 在线插件.id 在本地列表中存在:
    if 本地版本 != 在线版本:
      → 标记为"可更新"
    else:
      → 标记为"已安装"
  else:
    → 标记为"未安装"
```

**版本号比较策略**：精确字符串匹配，简单可靠。插件开发者通过更新 manifest.json 中的 version 字段管理版本。

### 3.4 与现有系统的集成

| 现有组件 | 集成方式 | 改动量 |
|---------|---------|--------|
| `PluginHost` (Rust) | 新增 `store.rs` 模块，复用 `install_from_zip()` | 小（新增文件） |
| `pluginStore` (Zustand) | 无需改动（在线商店独立管理状态） | 无 |
| `PluginManager` (TSX) | 新增"在线商店"标签页 | 中（新增文件 + 修改入口） |
| `PluginRegistry` (TS) | 无需改动（安装后自动发现） | 无 |
| `RightPanel` | 无需改动（PluginManager 入口不变） | 无 |
| `types/plugin.ts` | 新增 `OnlinePluginInfo` 等类型 | 小 |

---

## 4. 安全分析

### 4.1 风险矩阵

| 风险 | 等级 | 说明 | 缓解措施 |
|------|------|------|---------|
| 恶意插件提交到仓库 | **高** | 任何人都可 fork 并提交恶意插件 | 官方仓库审核制（PR review） |
| GitHub API 不可用 | **中** | GitHub 服务故障导致商店不可用 | 本地插件不受影响，商店降级提示 |
| API 速率限制 | **中** | 未认证用户每小时 60 次 | index.json 优先走 CDN 不消耗配额，仅安装消耗 |
| 插件版本混乱 | **低** | 开发者忘记更新 manifest.json 版本号 | 规范文档约束 + CI 可做版本检查 |

### 4.2 安全措施

#### 4.2.1 仓库安全

| 措施 | 说明 |
|------|------|
| PR 审核制 | 所有插件提交需经过审核合入 main 分支 |
| 分支保护 | main 分支禁止直接推送，必须通过 PR |
| CI 检查 | GitHub Actions 自动验证 manifest.json 格式和必填字段 |

#### 4.2.2 下载安全

| 措施 | 说明 | 实现成本 |
|------|------|---------|
| HTTPS 强制 | GitHub API 默认 HTTPS | 低（默认） |
| 文件大小限制 | 限制插件包最大 50MB | 低 |
| 沙箱联动 | 在线安装的插件自动受现有沙箱系统保护 | 低（现有） |

#### 4.2.3 沙箱联动

在线商店安装的插件**完全复用现有沙箱系统**，无额外安全风险：

```
在线安装的插件
  → 解压到 plugins_dir
  → PluginHost.discover() 发现
  → 沙箱验证 manifest.json（大小限制、字段校验、路径遍历检查）
  → 权限检查（未知权限拒绝、高风险权限警告）
  → 用户确认高风险权限后启用
```

---

## 5. 实现路线图

### 5.1 阶段划分

```
Phase 1: 基础能力（P0，预估 8-12h）
|-- 编写 .github/workflows/generate-index.yml（CI 自动索引）
|-- 在 server/market/plugins/ 下放置 1-2 个示例插件作为验证
|-- Rust: plugin_store_fetch_index 命令（CDN 优先 + raw 降级双模式）
|-- Rust: plugin_store_install 命令（下载 zipball + 调用 install_from_zip）
|-- 前端: OnlinePluginStore 组件（列表展示 + 安装按钮 + 刷新按钮）
|-- 前端: PluginManager 标签切换
+-- 端到端测试：从在线商店安装一个插件

Phase 2: 版本管理（P1，预估 4-6h）
|-- Rust: plugin_store_check_updates 命令
|-- Rust: plugin_store_update 命令（卸载旧版 + 安装新版）
|-- 前端: 版本对比展示（已安装/可更新/未安装）
|-- 前端: 一键更新按钮
+-- 前端: 批量更新（可选）

Phase 3: 搜索与体验增强（P1，预估 3-4h）
|-- 前端: 搜索过滤
|-- 前端: 按标签分类浏览
|-- 前端: 排序（按更新时间/名称）
|-- 前端: 加载状态和错误处理完善
+-- 前端: 安装进度展示
```

### 5.2 依赖关系

```
Phase 1（基础能力）
  +-- 依赖：现有 PluginHost.install_from_zip()
  +-- 依赖：reqwest HTTP 库（需确认是否已引入）
  +-- 依赖：CI 工作流配置

Phase 2（版本管理）
  +-- 依赖 Phase 1

Phase 3（搜索与体验增强）
  +-- 依赖 Phase 1
```

### 5.3 工时估算

| Phase | 组件 | 预估工时 |
|-------|------|---------|
| P1 | CI 工作流 + 示例插件 | 1-2h |
| P1 | Rust: plugin_store_fetch_index（双模式） | 2-3h |
| P1 | Rust: plugin_store_install | 3-4h |
| P1 | 前端: OnlinePluginStore 组件 | 2-3h |
| P2 | Rust: check_updates + update | 2-3h |
| P2 | 前端: 版本对比展示 | 2-3h |
| P3 | 前端: 搜索/分类/排序 | 3-4h |
| **合计** | | **15-22h** |

---

## 6. 关键决策清单

| # | 决策点 | 推荐方案 | 理由 |
|---|--------|---------|------|
| 1 | 分发方式 | **GitHub 仓库目录索引** | 插件代码直接可见，CI 自动生成索引，零手动维护 |
| 2 | 索引生成 | **GitHub Actions 自动扫描** | 开发者只需提交插件目录，索引自动更新 |
| 3 | 索引位置 | **`server/market/plugins/index.json`** | 放在 plugins/ 内，不污染外层其他板块 |
| 4 | 索引读取 | **CDN 优先 + raw 降级双模式** | CDN 加速日常使用，raw 保证实时性 |
| 5 | 强制刷新 | **用户点击"刷新"→ 直连 raw** | 绕过 CDN 缓存，获取最新数据 |
| 6 | 下载方式 | **GitHub API zipball** | 支持子目录下载，无需手动打包 |
| 7 | HTTP 客户端 | **reqwest**（如未引入则用 ureq） | 异步支持好，生态成熟 |
| 8 | 版本比较 | **精确字符串匹配** | 简单可靠 |
| 9 | 更新策略 | **手动更新（用户点击）** | 初期避免自动更新的复杂性 |
| 10 | 商店入口 | **PluginManager 标签切换** | 与现有 UI 无缝集成 |

---

## 7. 约束与风险

### 7.1 约束

| 约束 | 说明 |
|------|------|
| GitHub 依赖 | 插件商店依赖 GitHub 服务可用性，GitHub 不可用时商店不可用 |
| 网络要求 | 用户需要网络连接才能浏览和安装在线插件 |
| 目录命名约束 | 插件目录名 = 插件 ID，需唯一且符合 URL 安全字符 |
| 现有兼容 | 在线商店安装的插件与本地安装的插件行为完全一致 |

### 7.2 风险

| 风险 | 等级 | 缓解措施 |
|------|------|---------|
| GitHub API 速率限制 | **中** | index.json 优先走 CDN 不消耗配额；安装操作频率低，不易触达限制 |
| 插件质量参差不齐 | **中** | 官方仓库审核制，仅合入经过审查的插件 |
| 插件停止维护 | **低** | 用户可手动卸载，本地插件不受影响 |
| 网络下载失败 | **低** | 重试机制 + 用户友好的错误提示 |
| index.json 格式变更 | **低** | schemaVersion 字段，向前兼容 |

### 7.3 与现有架构的兼容性

| 现有特性 | 兼容性 | 说明 |
|---------|--------|------|
| 沙箱系统 | **完全兼容** | 在线安装的插件同样受沙箱保护 |
| 权限系统 | **完全兼容** | manifest.json 中的权限声明同样被检查 |
| 插件发现 | **完全兼容** | 安装到 plugins_dir 后自动被 discover() 发现 |
| 插件启用/禁用 | **完全兼容** | 与本地插件共用 enable/disable 逻辑 |
| 插件卸载 | **完全兼容** | 使用现有的 uninstall 逻辑 |
| 面板注册 | **完全兼容** | 安装后通过 PluginRegistry 自动注册 |
| 本地 zip 安装 | **完全兼容** | 在线商店作为补充，不影响现有安装方式 |

---

## 8. 总结

### 8.1 可行性结论

**结论：完全可行，且实现成本低。** 核心优势：

1. **零手动维护成本**：GitHub Actions 自动扫描 `server/market/plugins/` 目录生成 `index.json`，开发者只需提交插件目录
2. **代码天然可见**：每个插件是仓库中的一个目录，代码可直接浏览和审查
3. **安装机制已就绪**：`PluginHost.install_from_zip()` 已完整实现，GitHub API zipball 返回标准 zip，直接复用
4. **安全体系已就绪**：在线安装的插件自动受现有沙箱和权限系统保护
5. **双模式读取**：CDN 加速日常使用，raw 保证实时性，用户可手动强制刷新

### 8.2 核心工作量

| 模块 | 工作量 | 说明 |
|------|--------|------|
| CI 工作流 + 示例插件 | 1-2h | generate-index.yml + 验证 |
| Rust 侧（2 个命令） | 5-7h | fetch_index（双模式）+ install |
| 前端侧（1 个组件） | 2-3h | OnlinePluginStore + 标签切换 |
| **合计（Phase 1 核心）** | **8-12h** | 可交付最小可用版本 |

### 8.3 推荐执行顺序

```
Step 1: 配置 CI 工作流
  + 编写 .github/workflows/generate-index.yml
  + 在 server/market/plugins/ 下放置 1-2 个示例插件
  + 验证 CI 自动生成 server/market/index.json

Step 2: 实现 Rust 侧命令
  + plugin_store_fetch_index（CDN 优先 → raw 降级）
  + plugin_store_install（HTTP GET zipball → 临时文件 → install_from_zip → 清理）

Step 3: 实现前端组件
  + OnlinePluginStore（列表展示 + 搜索 + 安装 + 强制刷新）
  + PluginManager 标签切换（本地/在线）

Step 4: 端到端测试
  + 从在线商店浏览插件列表
  + 点击安装 → 下载 → 解压 → 自动发现 → 面板注册
  + 验证沙箱/权限系统正常工作
  + 验证 CDN 读取 + raw 降级 + 强制刷新
```

### 8.4 不推荐的功能（初期）

| 功能 | 原因 |
|------|------|
| 用户注册/登录 | 增加复杂度，GitHub 匿名访问即可 |
| 付费插件 | 需支付系统，与开源理念不符 |
| 自动更新 | 增加后台复杂度，手动更新更可控 |
| 插件评分/评论 | 需后端服务，初期可通过 GitHub Issues 替代 |
| 安装量统计 | 需后端服务，初期可通过 GitHub Insights/Star 查看 |

---

## 版本里程碑

| 版本 | 日期 | 变更说明 | Git Commit |
|------|------|---------|------------|
| v1.0 | 2026-06-21 | 在线插件商店可行性分析报告（文件夹方式资源库 + CI 自动索引 + raw/CDN 双模式） | `{{GIT_HASH}}` |
