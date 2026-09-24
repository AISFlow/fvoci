pub fn title_is_valid(title: &str) -> bool {
    let trimmed = title.trim();
    !trimmed.is_empty() && trimmed.chars().count() <= 500
}

pub fn task_type_is_valid(value: &str) -> bool {
    matches!(value, "task" | "subtask" | "milestone")
}

pub fn priority_is_valid(value: &str) -> bool {
    matches!(value, "none" | "low" | "medium" | "high" | "urgent")
}
