//! Account-wide (`GET /users/me/today`) and single-workspace (`GET /w/{ws}/today`)
//! `'Today'` hub logic.

use std::collections::HashMap;

use fvoci_model::{
    AccountToday, DiaryDate, OrgKind, Session, TODAY_SECTION_LIMIT, TimeZone, TodayWorkspaceMeta,
    WorkspaceRole, WorkspaceToday, add_days, day_at,
};
use fvoci_store::{Making, TodayWindow};

use super::App;
use crate::auth::now_millis;
use crate::error::AppError;

/// Window ahead of `now` included in upcoming course meetings on `'Today'` (48 hours).
const MEETINGS_LOOKAHEAD_MILLIS: i64 = 48 * 60 * 60 * 1_000;

impl App {
    /// Returns the `'Today'` hub summary for `user_id` inside `workspace_id`, evaluated
    /// on an organization-scoped handle.
    ///
    /// # Errors
    ///
    /// Returns 404 if `workspace_id` does not exist and the store's error if a query
    /// fails.
    pub async fn workspace_today(
        &self,
        workspace_id: &str,
        user_id: &str,
    ) -> Result<WorkspaceToday, AppError> {
        let now = now_millis();
        let today_str = self.user_today(user_id, now).await?;
        let today: DiaryDate = today_str
            .parse()
            .map_err(|_| AppError::internal("computed today date is invalid"))?;
        let settings = self.store().get_user_settings(user_id).await?;
        let soon_date = add_days(&today_str, 1).unwrap_or_else(|| today_str.clone());
        let sql_max_due_at = add_days(&soon_date, 1).unwrap_or_else(|| soon_date.clone());
        let tz = settings.time_zone;
        let window = TodayWindow {
            today,
            soon_date: &soon_date,
            sql_max_due_at: &sql_max_due_at,
            now_millis: now,
            meetings_until_millis: now.saturating_add(MEETINGS_LOOKAHEAD_MILLIS),
            day_of_millis: |ms| day_at(ms, &tz).or_else(|| day_at(ms, &TimeZone::default())),
        };

        let is_personal = self.store().is_own_space().await?;
        let ws = self.workspace(workspace_id).await?;
        let (org_id, org_name) = if is_personal {
            let own_org_id = self
                .store()
                .workspaces_of_user(user_id, Making::with_password())
                .await?
                .into_iter()
                .find(|entry| entry.id == workspace_id)
                .map_or_default(|entry| entry.org_id);
            (own_org_id, String::new())
        } else {
            let org = self.organization().await?;
            (org.id, org.name)
        };
        let meta = TodayWorkspaceMeta {
            workspace_id: workspace_id.to_owned(),
            workspace_name: if is_personal { String::new() } else { ws.title },
            org_id,
            org_name,
            is_personal,
        };
        Ok(self
            .store()
            .workspace_today(workspace_id, meta, user_id, &window)
            .await?)
    }

    /// Returns the account-wide `'Today'` hub summary across every workspace `session`
    /// currently opens, reporting `locked_workspace_count` for workspaces excluded by
    /// unfulfilled SSO or personal-space password requirements.
    ///
    /// # Errors
    ///
    /// Returns the store's error if a query fails.
    pub async fn account_today(&self, session: &Session) -> Result<AccountToday, AppError> {
        let user_id = session.user_id.as_str();
        let now = now_millis();
        let today_str = self.user_today(user_id, now).await?;
        let today: DiaryDate = today_str
            .parse()
            .map_err(|_| AppError::internal("computed today date is invalid"))?;
        let settings = self.store().get_user_settings(user_id).await?;
        let soon_date = add_days(&today_str, 1).unwrap_or_else(|| today_str.clone());
        let sql_max_due_at = add_days(&soon_date, 1).unwrap_or_else(|| soon_date.clone());
        let tz = settings.time_zone;
        let window = TodayWindow {
            today,
            soon_date: &soon_date,
            sql_max_due_at: &sql_max_due_at,
            now_millis: now,
            meetings_until_millis: now.saturating_add(MEETINGS_LOOKAHEAD_MILLIS),
            day_of_millis: |ms| day_at(ms, &tz).or_else(|| day_at(ms, &TimeZone::default())),
        };

        let workspaces = self.workspaces_of_user(session).await?;
        let mut acc = AccountToday::default();
        let mut org_names: HashMap<String, String> = HashMap::new();

        for ws in workspaces {
            if ws.role == WorkspaceRole::Guest {
                continue;
            }
            if ws.sso_required || ws.password_required {
                acc.locked_workspace_count = acc.locked_workspace_count.saturating_add(1);
                continue;
            }
            let Some(entered) = self.enter_workspace_quietly(session.into(), &ws.id).await? else {
                acc.locked_workspace_count = acc.locked_workspace_count.saturating_add(1);
                continue;
            };
            let is_personal = ws.kind == OrgKind::Personal;
            let org_name =
                resolve_org_name(&entered.app, &ws.org_id, ws.kind, &mut org_names).await;
            let meta = TodayWorkspaceMeta {
                workspace_id: ws.id.clone(),
                workspace_name: if is_personal { String::new() } else { ws.name },
                org_id: ws.org_id,
                org_name,
                is_personal,
            };
            let part = entered
                .app
                .store()
                .workspace_today(&ws.id, meta, user_id, &window)
                .await?;
            extend_account_today(&mut acc, part);
        }

        Ok(finish_account_today(acc, today_str))
    }
}

fn extend_account_today(acc: &mut AccountToday, part: WorkspaceToday) {
    acc.has_more_cards |= part.has_more_cards;
    acc.cards.extend(part.cards);
    acc.has_more_units |= part.has_more_units;
    acc.units.extend(part.units);
    acc.has_more_reviews |= part.has_more_reviews;
    acc.reviews.extend(part.reviews);
    acc.has_more_meetings |= part.has_more_meetings;
    acc.meetings.extend(part.meetings);
}

fn finish_account_today(mut acc: AccountToday, today: String) -> AccountToday {
    acc.today = today;
    sort_and_cap(&mut acc.cards, &mut acc.has_more_cards, |a, b| {
        a.due_date
            .cmp(&b.due_date)
            .then_with(|| a.card_id.cmp(&b.card_id))
    });
    sort_and_cap(&mut acc.units, &mut acc.has_more_units, |a, b| {
        a.unit
            .scheduled_date
            .cmp(&b.unit.scheduled_date)
            .then_with(|| a.unit.unit_id.cmp(&b.unit.unit_id))
    });
    sort_and_cap(&mut acc.reviews, &mut acc.has_more_reviews, |a, b| {
        a.review
            .scheduled_date
            .cmp(&b.review.scheduled_date)
            .then_with(|| a.review.id.cmp(&b.review.id))
    });
    sort_and_cap(&mut acc.meetings, &mut acc.has_more_meetings, |a, b| {
        a.starts_at
            .cmp(&b.starts_at)
            .then_with(|| a.meeting_id.cmp(&b.meeting_id))
    });
    acc
}

async fn resolve_org_name(
    org_app: &App,
    org_id: &str,
    kind: OrgKind,
    cache: &mut HashMap<String, String>,
) -> String {
    if kind == OrgKind::Personal {
        return String::new();
    }
    if let Some(cached) = cache.get(org_id) {
        return cached.clone();
    }
    let fetched = org_app.organization().await.map_or_default(|org| org.name);
    cache.insert(org_id.to_owned(), fetched.clone());
    fetched
}

fn sort_and_cap<T>(
    items: &mut Vec<T>,
    has_more: &mut bool,
    cmp: impl FnMut(&T, &T) -> std::cmp::Ordering,
) {
    items.sort_by(cmp);
    if items.len() > TODAY_SECTION_LIMIT {
        *has_more = true;
        items.truncate(TODAY_SECTION_LIMIT);
    }
}
