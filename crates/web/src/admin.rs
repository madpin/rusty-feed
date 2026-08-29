//! Admin views (M12): the LLM/embedding cost ledger.

use askama::Template;
use axum::extract::State;
use axum::response::{Html, IntoResponse, Response};
use axum_extra::extract::cookie::CookieJar;

use rustss_db::cost_ledger;

use crate::auth::csrf;
use crate::auth::extractor::AdminUser;
use crate::error::AppError;
use crate::prefs;
use crate::state::AppState;

struct CostRow {
    provider: String,
    model: String,
    kind: String,
    calls: i64,
    input_tokens: i64,
    output_tokens: i64,
    cost_usd: String,
}

#[derive(Template)]
#[template(path = "admin_costs.html")]
struct CostsTemplate {
    theme: String,
    user_email: String,
    csrf_token: String,
    ui: prefs::Preferences,
    rows: Vec<CostRow>,
    total_usd: String,
}

fn micro_to_usd(micro: i64) -> String {
    format!("${:.4}", micro as f64 / 1_000_000.0)
}

/// `GET /admin/costs` — cost ledger (admin only).
pub async fn costs(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let rows = cost_ledger::by_model(state.db.pool())
        .await?
        .into_iter()
        .map(|r| CostRow {
            provider: r.provider,
            model: r.model,
            kind: r.kind,
            calls: r.calls,
            input_tokens: r.input_tokens,
            output_tokens: r.output_tokens,
            cost_usd: micro_to_usd(r.cost_micro_usd),
        })
        .collect();
    let total_usd = micro_to_usd(cost_ledger::total_micro_usd(state.db.pool()).await?);

    let ui = prefs::load(&state.db, user.id).await;
    let (csrf_token, jar) = csrf::issue(jar, state.settings.cookie_secure);
    let body = CostsTemplate {
        theme: ui.theme.clone(),
        user_email: user.email,
        csrf_token,
        ui,
        rows,
        total_usd,
    }
    .render()
    .map(Html)
    .map_err(|e| AppError::Internal(anyhow::anyhow!("template render failed: {e}")))?;
    Ok((jar, body).into_response())
}
