use crate::commands::proxy::ProxyServiceState;

#[tauri::command]
pub async fn get_strict_pool_status(
    state: tauri::State<'_, ProxyServiceState>,
    page: Option<usize>,
    include_details: Option<bool>,
) -> Result<serde_json::Value, String> {
    let manager = {
        let instance = state.instance.read().await;
        if let Some(instance) = instance.as_ref() {
            instance.token_manager.clone()
        } else {
            let admin = state.admin_server.read().await;
            admin
                .as_ref()
                .ok_or("Proxy has not initialized")?
                .axum_server
                .token_manager
                .clone()
        }
    };
    pool_status(manager, page.unwrap_or(0), include_details.unwrap_or(true)).await
}

pub async fn pool_status(
    manager: std::sync::Arc<crate::proxy::TokenManager>,
    page: usize,
    include_details: bool,
) -> Result<serde_json::Value, String> {
    manager.sync_pending_accounts().await;
    tokio::task::spawn_blocking(move || {
        manager.cleanup_strict_sessions();
        let mut value = if include_details {
            manager.strict_pool_status(page)
        } else {
            manager.strict_pool_status_summary()
        };
        value["runtime"] = crate::proxy::runtime_limits::LIMITS.info();
        value
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn export_delete_forbidden() -> Result<serde_json::Value, String> {
    tokio::task::spawn_blocking(|| {
        let accounts = crate::modules::account::list_accounts()?;
        let forbidden: Vec<_> = accounts
            .into_iter()
            .filter(|account| {
                account
                    .quota
                    .as_ref()
                    .is_some_and(|quota| quota.is_forbidden)
            })
            .collect();
        let exported: Vec<_> = forbidden
            .iter()
            .map(|account| {
                serde_json::json!({
                    "id": account.id,
                    "name": account.name,
                    "email": account.email,
                    "refresh_token": account.token.refresh_token,
                })
            })
            .collect();
        let ids: Vec<String> = forbidden.into_iter().map(|account| account.id).collect();
        if !ids.is_empty() {
            crate::modules::account::delete_accounts(&ids)?;
        }
        Ok(serde_json::json!({
            "accounts": exported,
            "deleted": ids.len(),
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}
