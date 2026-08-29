pub(crate) fn content_size(value: Option<f64>) -> String {
    match value {
        Some(value) if value >= 1_000.0 => format!("~{:.1}k chars", value / 1_000.0),
        Some(value) => format!("~{} chars", value.round() as i64),
        None => "No RSS sample yet".to_string(),
    }
}

pub(crate) fn frequency(value: Option<f64>) -> String {
    match value {
        Some(hours) if hours < 1.5 => "About hourly".to_string(),
        Some(hours) if hours < 8.0 => "Several times daily".to_string(),
        Some(hours) if hours < 36.0 => "About daily".to_string(),
        Some(hours) if hours < 24.0 * 8.0 => "About weekly".to_string(),
        Some(hours) if hours < 24.0 * 35.0 => "About monthly".to_string(),
        Some(hours) => format!("About every {} days", (hours / 24.0).round() as i64),
        None => "Frequency not sampled".to_string(),
    }
}
