//! Storage for `user_settings` and `user_client_state`.

use super::{
    BTreeMap, CLIENT_STATE_MAX_ENTRIES, ClientStatePatch, Id, Param, Scope, StoreError,
    USER_SETTINGS_COLUMNS, UserSettings, UserSettingsPatch, lock_user,
};

pub(super) async fn stored_settings(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<UserSettings, StoreError> {
    let mut sql = scope.sql([
        "SELECT ",
        USER_SETTINGS_COLUMNS,
        " FROM user_settings WHERE user_id = ",
    ]);
    sql.bind(Id(user_id));
    let Some(row) = scope.fetch_optional(sql).await? else {
        return Ok(UserSettings::default());
    };
    Ok(UserSettings {
        language: row.text(0)?.parse().unwrap_or_default(),
        theme: row.text(1)?.parse().unwrap_or_default(),
        density: row.text(2)?.parse().unwrap_or_default(),
        card_open: row.text(3)?.parse().unwrap_or_default(),
        haptics: row.bool(4)?,
        week_start: row.text(5)?.parse().unwrap_or_default(),
        hour_cycle: row.text(6)?.parse().unwrap_or_default(),
        time_zone: row.text(7)?.parse().unwrap_or_default(),
        notify_mentions: row.bool(8)?,
        notify_assignments: row.bool(9)?,
        notify_comments: row.bool(10)?,
        notify_due: row.bool(11)?,
    })
}

/// Maps a setting and optional stored value (`None` resets to the default) to its
/// `user_settings` column and typed SQL parameter.
fn setting_column_param(
    name: fvoci_model::SettingName,
    value: Option<&str>,
) -> (&'static str, Param) {
    use fvoci_model::SettingName;
    let text = |default: &'static str| Param::Text(value.unwrap_or(default).to_owned());
    let flag = |default: bool| Param::Bool(value.map_or(default, |stored| stored == "true"));
    match name {
        SettingName::Language => ("language", text("auto")),
        SettingName::Theme => ("theme", text("auto")),
        SettingName::Density => ("density", text("auto")),
        SettingName::CardOpen => ("card_open", text("center")),
        SettingName::Haptics => ("haptics", flag(true)),
        SettingName::WeekStart => ("week_start", text("auto")),
        SettingName::HourCycle => ("hour_cycle", text("auto")),
        SettingName::TimeZone => ("time_zone", text("auto")),
        SettingName::NotifyMentions => ("notify_mentions", flag(true)),
        SettingName::NotifyAssignments => ("notify_assignments", flag(true)),
        SettingName::NotifyComments => ("notify_comments", flag(true)),
        SettingName::NotifyDue => ("notify_due", flag(true)),
    }
}

pub(super) async fn upsert_user_settings(
    scope: &mut Scope<'_>,
    user_id: &str,
    patch: &UserSettingsPatch,
) -> Result<(), StoreError> {
    let (set, reset) = patch.changes();
    let mut columns: Vec<(&'static str, Param)> = Vec::with_capacity(set.len() + reset.len());
    for (name, value) in set {
        columns.push(setting_column_param(name, Some(value)));
    }
    for name in reset {
        columns.push(setting_column_param(name, None));
    }
    if columns.is_empty() {
        return Ok(());
    }
    let names: Vec<&'static str> = columns.iter().map(|(col, _)| *col).collect();
    let mut sql = scope.sql("INSERT INTO user_settings (user_id");
    for col in &names {
        sql.push(", ").push(*col);
    }
    sql.push(") VALUES (").bind(Id(user_id));
    for (_, param) in columns {
        sql.push_bind(", ", param);
    }
    sql.push(") ON CONFLICT (user_id) DO UPDATE SET ");
    for (index, col) in names.iter().enumerate() {
        if index > 0 {
            sql.push(", ");
        }
        sql.push(*col).push(" = EXCLUDED.").push(*col);
    }
    scope.execute(sql).await?;
    Ok(())
}

pub(crate) async fn get_client_state(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<BTreeMap<String, String>, StoreError> {
    let mut sql = scope.sql("SELECT key, value FROM user_client_state WHERE user_id = ");
    sql.bind(Id(user_id)).push(" ORDER BY key");
    scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| Ok((row.text(0)?, row.text(1)?)))
        .collect()
}

pub(crate) async fn patch_client_state(
    scope: &mut Scope<'_>,
    user_id: &str,
    patch: &ClientStatePatch,
) -> Result<BTreeMap<String, String>, StoreError> {
    lock_user(scope, user_id).await?;
    for (key, value) in &patch.updated_fields {
        let mut sql = scope.sql("INSERT INTO user_client_state (user_id, key, value) VALUES (");
        sql.bind(Id(user_id))
            .push_bind(", ", key)
            .push_bind(", ", value)
            .push(") ON CONFLICT (user_id, key) DO UPDATE SET value = EXCLUDED.value");
        scope.execute(sql).await?;
    }
    for key in &patch.deleted_fields {
        let mut sql = scope.sql("DELETE FROM user_client_state WHERE user_id = ");
        sql.bind(Id(user_id)).push_bind(" AND key = ", key);
        scope.execute(sql).await?;
    }
    let entries = get_client_state(scope, user_id).await?;
    if entries.len() > CLIENT_STATE_MAX_ENTRIES {
        return Err(StoreError::TooManyClientStateEntries {
            limit: CLIENT_STATE_MAX_ENTRIES,
        });
    }
    Ok(entries)
}
