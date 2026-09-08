//! cc-switch-remote 出借方种子工具（容器化/自动化测试用）
//!
//! 用法：`seed_lender <seed.json>`
//!
//! 在应用数据目录（~/.cc-switch-remote）中写入：
//! 1. 一个供应商（provider）
//! 2. 组网网络配置（share_network，白名单包含该供应商）
//! 3. share key 降级密钥文件（share/secret-<hash>）
//!
//! 应用启动时 `restore_from_db` 会自动恢复网络并开始出借。
//!
//! seed.json 格式：
//! ```json
//! {
//!   "shareId": "ZHIPU888",
//!   "shareKey": "<base64url 32字节>",
//!   "nodeName": "lender-container",
//!   "provider": {
//!     "id": "zhipu",
//!     "appType": "claude",
//!     "name": "智谱 GLM",
//!     "settingsConfig": { "env": { "ANTHROPIC_BASE_URL": "...", "ANTHROPIC_AUTH_TOKEN": "...", "ANTHROPIC_MODEL": "glm-5.3" } }
//!   }
//! }
//! ```

use cc_switch_lib::database::{Database, ShareNetworkRow};
use cc_switch_lib::provider::Provider;
use cc_switch_lib::share::auth::share_id_hash;
use serde_json::Value;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("用法: seed_lender <seed.json>");
        std::process::exit(2);
    });
    if let Err(e) = run(&path) {
        eprintln!("[seed] 失败: {e}");
        std::process::exit(1);
    }
}

fn run(path: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("读取 seed.json 失败: {e}"))?;
    let seed: Value =
        serde_json::from_str(&text).map_err(|e| format!("解析 seed.json 失败: {e}"))?;

    let share_id = seed
        .get("shareId")
        .and_then(|v| v.as_str())
        .ok_or("缺少 shareId")?
        .to_string();
    let share_key = seed
        .get("shareKey")
        .and_then(|v| v.as_str())
        .ok_or("缺少 shareKey")?
        .to_string();
    let node_name = seed
        .get("nodeName")
        .and_then(|v| v.as_str())
        .unwrap_or("lender-node")
        .to_string();
    let provider_seed = seed.get("provider").ok_or("缺少 provider")?;
    let provider_id = provider_seed
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("缺少 provider.id")?
        .to_string();
    let app_type = provider_seed
        .get("appType")
        .and_then(|v| v.as_str())
        .unwrap_or("claude")
        .to_string();
    let provider_name = provider_seed
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or(&provider_id)
        .to_string();
    let settings_config = provider_seed
        .get("settingsConfig")
        .cloned()
        .ok_or("缺少 provider.settingsConfig")?;

    let hash = share_id_hash(&share_id);
    let db = Database::init().map_err(|e| format!("初始化数据库失败: {e}"))?;

    // 1. 写入供应商
    let provider = Provider::with_id(
        provider_id.clone(),
        provider_name.clone(),
        settings_config,
        None,
    );
    db.save_provider(&app_type, &provider)
        .map_err(|e| format!("写入供应商失败: {e}"))?;

    // 2. 写入组网网络配置（白名单包含该供应商）
    let now = chrono::Utc::now().timestamp();
    let row = ShareNetworkRow {
        share_id: share_id.clone(),
        share_id_hash: hash.clone(),
        role: "creator".to_string(),
        route_preference: "network_first".to_string(),
        relay_addr: None,
        shared_provider_ids: vec![format!("{app_type}:{provider_id}")],
        quota_scope: "daily".to_string(),
        quota_max_tokens: 0,
        quota_per_peer: true,
        node_name: node_name.clone(),
        created_at: now,
        updated_at: now,
    };
    db.save_share_network(&row)
        .map_err(|e| format!("写入组网配置失败: {e}"))?;

    // 3. 写入 share key 降级密钥文件（0600）
    let share_dir = cc_switch_lib::share::config::share_data_dir();
    let secret_path = share_dir.join(format!("secret-{hash}"));
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true).mode(0o600);
        let mut file = opts
            .open(&secret_path)
            .map_err(|e| format!("写入密钥文件失败: {e}"))?;
        file.write_all(share_key.as_bytes())
            .map_err(|e| format!("写入密钥文件失败: {e}"))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&secret_path, &share_key).map_err(|e| format!("写入密钥文件失败: {e}"))?;
    }

    println!("[seed] 完成:");
    println!("  share_id   = {share_id}");
    println!("  供应商     = {provider_name} ({app_type}:{provider_id})");
    println!("  白名单     = {app_type}:{provider_id}");
    println!("  密钥文件   = {}", secret_path.display());
    println!("  节点名     = {node_name}");
    Ok(())
}
