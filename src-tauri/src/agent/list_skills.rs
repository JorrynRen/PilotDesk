//! Agent 技能模块：技能根解析、扫描、安装/卸载、主文件读写
//!
//! 技能的落地形态是「**一个目录 + 入口文件**（默认 `SKILL.md`）」；识别不靠目录名，
//! 靠入口文件的 YAML frontmatter（`name` / `description`）。
//!
//! 安全约定：所有"按路径操作"的入口（卸载 / 读 / 写）都必须先过 [`ensure_within`]，
//! 目标必须落在该 Agent 的技能根目录内 —— 否则前端传来的路径可以删改任意文件。

use std::path::{Path, PathBuf};

use crate::db::models::SkillInfo;

/// 技能入口文件默认名（Agent 未单独配置 `skill_entry_file` 时的口径）
pub const DEFAULT_ENTRY_FILE: &str = "SKILL.md";

/// 获取用户 home 目录（跨平台）
pub fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

/// 解析技能根目录：支持 `{agent_type}` 占位与 `~/` 前缀（与 Agent 配置里的口径一致）
pub fn resolve_skills_dir(skills_dir: &str, agent_type: &str) -> Option<PathBuf> {
    let resolved = skills_dir.replace("{agent_type}", agent_type);
    if let Some(rest) = resolved.strip_prefix("~/") {
        Some(home_dir()?.join(rest))
    } else {
        Some(PathBuf::from(resolved))
    }
}

/// 解析入口文件内容的 frontmatter，返回 (name, description)；不是合法技能返回 None
pub fn parse_skill_content(raw: &str) -> Option<(String, String)> {
    let raw = raw.trim();
    if !raw.starts_with("---") {
        return None;
    }
    // 找到第二个 "---"
    let end = raw[3..].find("---")?;
    let frontmatter = &raw[3..3 + end];

    // 用 serde_yaml 解析 frontmatter
    let value: serde_yaml::Value = serde_yaml::from_str(frontmatter).ok()?;
    let mapping = value.as_mapping()?;

    let name = mapping
        .get(&serde_yaml::Value::String("name".into()))?
        .as_str()?
        .trim()
        .to_string();
    let description = mapping
        .get(&serde_yaml::Value::String("description".into()))?
        .as_str()?
        .trim_matches('"')
        .trim()
        .to_string();
    if name.is_empty() {
        return None;
    }
    Some((name, description))
}

/// 解析单个技能入口文件（带路径信息）
pub fn parse_skill_md(path: &Path) -> Option<SkillInfo> {
    let raw = std::fs::read_to_string(path).ok()?;
    let (name, description) = parse_skill_content(&raw)?;
    let dir = path.parent().unwrap_or_else(|| Path::new(""));
    Some(SkillInfo::new(&name, &description, "").with_paths(dir, path))
}

/// 扫描技能目录
///
/// display_mode: recursive（递归显示全部）或 collection（只显示集合名）
pub fn scan_skills_dir(skills_dir: &Path, entry_file: &str, display_mode: &str) -> Vec<SkillInfo> {
    if !skills_dir.exists() || !skills_dir.is_dir() {
        return vec![];
    }

    let mut skills = Vec::new();

    // 如果当前目录有入口文件，直接解析并返回
    let own_skill = skills_dir.join(entry_file);
    if own_skill.exists() {
        if let Some(info) = parse_skill_md(&own_skill) {
            skills.push(info);
        }
        // collection 模式：只显示集合名，不递归子目录
        if display_mode == "collection" {
            return skills;
        }
        // recursive 模式：解析入口文件后继续递归子目录
        if display_mode != "recursive" {
            return skills;
        }
    }

    // 递归遍历子目录
    let entries = match std::fs::read_dir(skills_dir) {
        Ok(e) => e,
        Err(_) => return skills,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            skills.extend(scan_skills_dir(&path, entry_file, display_mode));
        }
    }
    skills
}

// ════════════════════════════════════════════════════════════
// 路径安全
// ════════════════════════════════════════════════════════════

/// 规范化 `target` 并确认它落在 `root` 内（且不是 `root` 本身）。
///
/// 返回规范化后的绝对路径，供后续 fs 操作使用。软链接会被 canonicalize 解开，
/// 因此指向技能根外部的软链接同样会被拒绝。
pub fn ensure_within(root: &Path, target: &Path) -> Result<PathBuf, String> {
    let root_c = root
        .canonicalize()
        .map_err(|e| format!("技能目录不可用（{}）: {}", root.display(), e))?;
    let target_c = target
        .canonicalize()
        .map_err(|e| format!("路径不存在（{}）: {}", target.display(), e))?;
    if target_c == root_c {
        return Err("不能对技能根目录本身执行该操作".to_string());
    }
    if !target_c.starts_with(&root_c) {
        return Err("路径越界：目标不在该技能目录内".to_string());
    }
    Ok(target_c)
}

/// 目录名净化：拒绝路径分隔符与保留字，避免 `../` 或绝对路径逃出技能根
fn safe_dir_name(raw: &str) -> Option<String> {
    let name = raw.trim().trim_end_matches(['/', '\\']);
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    if name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
        return None;
    }
    Some(name.to_string())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst)
        .map_err(|e| format!("创建目录失败（{}）: {}", dst.display(), e))?;
    let entries =
        std::fs::read_dir(src).map_err(|e| format!("读取目录失败（{}）: {}", src.display(), e))?;
    for entry in entries.flatten() {
        let p = entry.path();
        // 不跟随软链接：避免把技能根外部的目录/文件"复制"进来
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if ft.is_symlink() {
            continue;
        }
        let target = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_recursive(&p, &target)?;
        } else if ft.is_file() {
            std::fs::copy(&p, &target)
                .map_err(|e| format!("复制文件失败（{}）: {}", p.display(), e))?;
        }
    }
    Ok(())
}

/// 解压 zip 到 `dest`（每一项都按 `enclosed_name` 校验，防 zip-slip）
fn extract_zip_to(zip_path: &Path, dest: &Path) -> Result<(), String> {
    use std::io::Read;

    let file = std::fs::File::open(zip_path)
        .map_err(|e| format!("打开压缩包失败（{}）: {}", zip_path.display(), e))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("压缩包无法解析: {}", e))?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("读取压缩包条目失败: {}", e))?;
        let rel = entry
            .enclosed_name()
            .ok_or_else(|| format!("压缩包内含非法路径: {}", entry.name()))?
            .to_path_buf();
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)
                .map_err(|e| format!("创建目录失败（{}）: {}", out.display(), e))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建目录失败（{}）: {}", parent.display(), e))?;
        }
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| format!("读取压缩包内容失败: {}", e))?;
        std::fs::write(&out, buf)
            .map_err(|e| format!("写入文件失败（{}）: {}", out.display(), e))?;
    }
    Ok(())
}

/// 在解压结果里定位技能目录：
/// 1) 根目录自身就是技能；2) 只有唯一顶层目录且它是技能；3) 递归（限深 3）找第一个技能
fn locate_skill_dir(root: &Path, entry_file: &str) -> Option<PathBuf> {
    if parse_skill_md(&root.join(entry_file)).is_some() {
        return Some(root.to_path_buf());
    }
    let top: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.path())
        .collect();
    if top.len() == 1 {
        if parse_skill_md(&top[0].join(entry_file)).is_some() {
            return Some(top[0].clone());
        }
    }
    find_skill_dir_bounded(root, entry_file, 3)
}

fn find_skill_dir_bounded(dir: &Path, entry_file: &str, depth: u32) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }
    let entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .collect();
    for p in entries.iter().filter(|p| p.is_dir()) {
        if parse_skill_md(&p.join(entry_file)).is_some() {
            return Some(p.clone());
        }
    }
    for p in entries.iter().filter(|p| p.is_dir()) {
        if let Some(found) = find_skill_dir_bounded(p, entry_file, depth - 1) {
            return Some(found);
        }
    }
    None
}

// ════════════════════════════════════════════════════════════
// 安装 / 卸载 / 主文件读写
// ════════════════════════════════════════════════════════════

/// 安装技能：`source` 可以是技能目录，也可以是 .zip 包。
///
/// 目标目录名优先取技能目录名（zip 取包内那一层目录名），已存在同名目录时拒绝覆盖。
/// 复制/解压后必须能解析入口文件，否则回滚并报错（不留半个技能）。
pub fn install_skill(
    skills_dir: &Path,
    entry_file: &str,
    source: &Path,
) -> Result<SkillInfo, String> {
    if !source.exists() {
        return Err(format!("来源不存在: {}", source.display()));
    }
    std::fs::create_dir_all(skills_dir)
        .map_err(|e| format!("技能目录不可用（{}）: {}", skills_dir.display(), e))?;

    if source.is_dir() {
        // 目录来源：要求它本身就是技能目录（入口文件可直接解析），不做"批量安装"
        if parse_skill_md(&source.join(entry_file)).is_none() {
            return Err(format!(
                "该目录不是技能目录：缺少可解析的 {}（需含 --- frontmatter 与 name/description）",
                entry_file
            ));
        }
        let name = safe_dir_name(
            source
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("skill"),
        )
        .ok_or_else(|| "技能目录名不合法".to_string())?;
        let dest = skills_dir.join(&name);
        // 来源已在技能目录内（或就是技能根）→ 拒绝：目标会落在来源里，递归复制会自我嵌套
        if let (Ok(src_c), Ok(root_c)) = (source.canonicalize(), skills_dir.canonicalize()) {
            if src_c == root_c || src_c.starts_with(&root_c) {
                return Err("该技能已在技能目录内，无需安装".to_string());
            }
        }
        if dest.exists() {
            return Err(format!("技能目录「{}」已存在，请先卸载或改名", name));
        }
        copy_skill_into(source, &dest, entry_file)
    } else {
        // zip 来源：先解到临时目录 → 定位技能目录 → 复制到位；失败一律清理，不留半个技能
        let stem = safe_dir_name(
            source
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("skill"),
        )
        .ok_or_else(|| "压缩包名不合法".to_string())?;
        let tmp = skills_dir.join(format!(".install-tmp-{}", stem));
        if tmp.exists() {
            let _ = std::fs::remove_dir_all(&tmp);
        }
        std::fs::create_dir_all(&tmp).map_err(|e| format!("创建临时目录失败: {}", e))?;

        let result = (|| -> Result<SkillInfo, String> {
            extract_zip_to(source, &tmp)?;
            let skill_src = locate_skill_dir(&tmp, entry_file)
                .ok_or_else(|| format!("压缩包里没找到技能目录（需含 {}）", entry_file))?;
            // 包内根目录本身就是技能时用包名做目录名，否则用包内那层目录名
            let name = if skill_src == tmp {
                stem.clone()
            } else {
                safe_dir_name(
                    skill_src
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(&stem),
                )
                .unwrap_or_else(|| stem.clone())
            };
            let dest = skills_dir.join(&name);
            if dest.exists() {
                return Err(format!("技能目录「{}」已存在，请先卸载或改名", name));
            }
            copy_skill_into(&skill_src, &dest, entry_file)
        })();

        let _ = std::fs::remove_dir_all(&tmp);
        result
    }
}

/// 复制技能目录到位并校验入口文件；任何一步失败都把目标目录清掉（含复制中途失败）
fn copy_skill_into(source: &Path, dest: &Path, entry_file: &str) -> Result<SkillInfo, String> {
    if let Err(e) = copy_dir_recursive(source, dest) {
        let _ = std::fs::remove_dir_all(dest);
        return Err(e);
    }
    match parse_skill_md(&dest.join(entry_file)) {
        Some(info) => Ok(info),
        None => {
            let _ = std::fs::remove_dir_all(dest);
            Err("安装后无法解析入口文件，已回滚".to_string())
        }
    }
}

/// 卸载技能（删除该技能目录；破坏性操作，调用方需二次确认）
pub fn uninstall_skill(skills_dir: &Path, dir_path: &Path) -> Result<(), String> {
    let target = ensure_within(skills_dir, dir_path)?;
    if !target.is_dir() {
        return Err("目标不是技能目录".to_string());
    }
    std::fs::remove_dir_all(&target).map_err(|e| format!("删除技能目录失败: {}", e))
}

/// 读取技能主文件
pub fn read_entry(skills_dir: &Path, entry_path: &Path) -> Result<String, String> {
    let target = ensure_within(skills_dir, entry_path)?;
    if !target.is_file() {
        return Err("技能主文件不存在或不是文件".to_string());
    }
    std::fs::read_to_string(&target).map_err(|e| format!("读取技能主文件失败: {}", e))
}

/// 写回技能主文件（保存前校验 frontmatter，避免把手改坏成扫不出来的技能）
pub fn write_entry(skills_dir: &Path, entry_path: &Path, content: &str) -> Result<(), String> {
    let target = ensure_within(skills_dir, entry_path)?;
    if parse_skill_content(content).is_none() {
        return Err(
            "内容不是合法的技能主文件：需要 `---` 包裹的 frontmatter，且含非空的 name 与 description"
                .to_string(),
        );
    }
    std::fs::write(&target, content).map_err(|e| format!("写入技能主文件失败: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pd-skill-test-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_skill(dir: &Path, name: &str, skill_name: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!(
                "---\nname: {}\ndescription: 测试技能\n---\n\n正文 {}\n",
                skill_name, name
            ),
        )
        .unwrap();
    }

    #[test]
    fn parse_content_requires_frontmatter_and_name() {
        assert!(parse_skill_content("没有 frontmatter").is_none());
        assert!(parse_skill_content("---\ndescription: 只有描述\n---\n").is_none());
        let ok = parse_skill_content("---\nname: demo\ndescription: 演示\n---\n正文").unwrap();
        assert_eq!(ok.0, "demo");
        assert_eq!(ok.1, "演示");
    }

    #[test]
    fn install_from_dir_copies_and_fills_paths() {
        let root = tmp_root("install");
        let src = root.join("src-demo");
        write_skill(&src, "demo", "demo-skill");
        let skills_root = root.join("skills");

        let info = install_skill(&skills_root, "SKILL.md", &src).unwrap();
        assert_eq!(info.name, "demo-skill");
        assert!(Path::new(&info.entry_path).is_file());
        assert_eq!(
            Path::new(&info.dir_path)
                .file_name()
                .and_then(|n| n.to_str()),
            Some("src-demo")
        );
        // 同名再装一次要拒绝（不静默覆盖）
        assert!(install_skill(&skills_root, "SKILL.md", &src).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn install_rejects_non_skill_dir() {
        let root = tmp_root("reject");
        let src = root.join("not-a-skill");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("README.md"), "hi").unwrap();
        let err = install_skill(&root.join("skills"), "SKILL.md", &src).unwrap_err();
        assert!(err.contains("不是技能目录"), "err = {}", err);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn install_from_zip_unwraps_single_top_dir() {
        use std::io::Write;

        let root = tmp_root("zip");
        // 造一个 zip：demo/SKILL.md —— 外层包了一层目录，这是"打包技能目录"的常见形态
        let zip_path = root.join("demo-skill.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
        writer.start_file("demo/SKILL.md", opts).unwrap();
        writer
            .write_all("---\nname: zip-demo\ndescription: 压缩包技能\n---\n\n正文\n".as_bytes())
            .unwrap();
        writer.finish().unwrap();

        let skills_root = root.join("skills");
        let info = install_skill(&skills_root, "SKILL.md", &zip_path).unwrap();
        assert_eq!(info.name, "zip-demo");
        assert!(skills_root.join("demo").join("SKILL.md").is_file());
        // 临时解压目录不能残留
        assert!(!skills_root.join(".install-tmp-demo-skill").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ensure_within_blocks_escape_and_root_itself() {
        let root = tmp_root("within");
        let skills_root = root.join("skills");
        let inside = skills_root.join("a");
        write_skill(&inside, "a", "a");
        let outside = root.join("outside");
        std::fs::create_dir_all(&outside).unwrap();

        // 根目录本身、根外目录都要拒绝
        assert!(ensure_within(&skills_root, &skills_root).is_err());
        assert!(ensure_within(&skills_root, &outside).is_err());
        // 根内目录通过
        assert!(ensure_within(&skills_root, &inside).is_ok());
        // 卸载也只能删根内目录
        assert!(uninstall_skill(&skills_root, &outside).is_err());
        assert!(uninstall_skill(&skills_root, &inside).is_ok());
        assert!(!inside.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn write_entry_rejects_broken_frontmatter() {
        let root = tmp_root("write");
        let skills_root = root.join("skills");
        let dir = skills_root.join("a");
        write_skill(&dir, "a", "a");
        let entry = dir.join("SKILL.md");

        assert!(write_entry(&skills_root, &entry, "手滑删了 frontmatter").is_err());
        // 原内容保持不变
        assert!(parse_skill_md(&entry).is_some());

        let good = "---\nname: a\ndescription: 改过了\n---\n新正文\n";
        write_entry(&skills_root, &entry, good).unwrap();
        assert_eq!(read_entry(&skills_root, &entry).unwrap(), good);
        let _ = std::fs::remove_dir_all(&root);
    }
}
