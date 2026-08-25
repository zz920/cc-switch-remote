//! 出借方用量限额
//!
//! 计量口径：proxy_request_logs 中 provider_id 形如 `sharelend:<peer>:<id>` 的行，
//! token 总量 = input + output + cache_read + cache_creation（按周期起点聚合）。

use crate::database::Database;
use crate::error::AppError;

/// 限额检查结果
#[derive(Debug, Clone, Copy)]
pub struct QuotaCheck {
    pub allowed: bool,
    pub used: i64,
    /// None = 不限额
    pub remaining: Option<i64>,
}

/// 计算限额周期的起点（本地时区）
///
/// - `daily`：今日 00:00
/// - `monthly`：本月 1 日 00:00
pub fn period_start_epoch(scope: &str) -> i64 {
    use chrono::{Datelike, Local, TimeZone};
    let now = Local::now();
    let start = if scope == "monthly" {
        Local
            .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
            .single()
    } else {
        Local
            .with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0)
            .single()
    };
    start.map(|t| t.timestamp()).unwrap_or(0)
}

/// 检查某 peer（或全网）是否还有出借配额
///
/// - `max_tokens == 0`：不限额，直接放行
/// - `per_peer == true`：按该 peer 单独计量
/// - `per_peer == false`：全网络共享一个总额度
pub fn check_lend_quota(
    db: &Database,
    peer_id: &str,
    scope: &str,
    max_tokens: i64,
    per_peer: bool,
) -> Result<QuotaCheck, AppError> {
    if max_tokens <= 0 {
        return Ok(QuotaCheck {
            allowed: true,
            used: 0,
            remaining: None,
        });
    }
    let since = period_start_epoch(scope);
    let used = if per_peer {
        db.share_peer_tokens_used(peer_id, since)?
    } else {
        db.share_all_peers_tokens_used(since)?
            .into_iter()
            .map(|(_, t)| t)
            .sum()
    };
    let remaining = max_tokens - used;
    Ok(QuotaCheck {
        allowed: remaining > 0,
        used,
        remaining: Some(remaining.max(0)),
    })
}

/// 查询某 peer 本周期的已用量与剩余量（状态展示用）
pub fn peer_quota_status(
    db: &Database,
    peer_id: &str,
    scope: &str,
    max_tokens: i64,
) -> Result<(i64, Option<i64>), AppError> {
    let since = period_start_epoch(scope);
    let used = db.share_peer_tokens_used(peer_id, since)?;
    let remaining = if max_tokens > 0 {
        Some((max_tokens - used).max(0))
    } else {
        None
    };
    Ok((used, remaining))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_start_is_midnight_or_month_start() {
        let daily = period_start_epoch("daily");
        let monthly = period_start_epoch("monthly");
        assert!(daily > 0 && monthly > 0);
        assert!(monthly <= daily);
    }

    #[test]
    fn unlimited_quota_always_allows() {
        let db = Database::memory().unwrap();
        let check = check_lend_quota(&db, "peer-x", "daily", 0, true).unwrap();
        assert!(check.allowed);
        assert!(check.remaining.is_none());
    }

    #[test]
    fn quota_blocks_after_limit() {
        let db = Database::memory().unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, input_tokens,
                    output_tokens, latency_ms, status_code, created_at
                 ) VALUES ('q1', 'sharelend:peer-x:p1', 'claude', 'm', 60, 40, 1, 200, ?1)",
                [period_start_epoch("daily") + 10],
            )
            .unwrap();
        }
        // 已用 100；限额 150 → 放行余 50
        let check = check_lend_quota(&db, "peer-x", "daily", 150, true).unwrap();
        assert!(check.allowed);
        assert_eq!(check.used, 100);
        assert_eq!(check.remaining, Some(50));
        // 限额 100 → 用尽，拒绝
        let check = check_lend_quota(&db, "peer-x", "daily", 100, true).unwrap();
        assert!(!check.allowed);
        // 全网口径：peer-y 没用过，但全网额度已包含 peer-x 的 100
        let check = check_lend_quota(&db, "peer-y", "daily", 100, false).unwrap();
        assert!(!check.allowed);
    }
}
