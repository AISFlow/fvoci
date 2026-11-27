//! Boards and board membership.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::JsonMap;
use crate::allowlist::holds_nul;
use crate::board_ops::DoneWhen;
use crate::card_property::{
    CardPropertyDefinition, CardPropertyError, PersonProperties, PropertyType,
    validate_card_property_definitions,
};

/// Who may see and join a board.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ContainerVisibility {
    /// Any member of the workspace may find and join the board.
    #[default]
    Open,
    /// Only invited users may access the board.
    Private,
}

impl ContainerVisibility {
    /// Every visibility there is.
    pub const ALL: [Self; 2] = [Self::Open, Self::Private];

    /// Returns the wire and database form.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Private => "private",
        }
    }
}

impl std::fmt::Display for ContainerVisibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when a string names no board visibility.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0:?} is no board visibility")]
pub struct ParseContainerVisibilityError(String);

impl FromStr for ContainerVisibility {
    type Err = ParseContainerVisibilityError;

    /// Reads the wire and database form ([`ContainerVisibility::as_str`]), exactly.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|visibility| visibility.as_str() == text)
            .ok_or_else(|| ParseContainerVisibilityError(text.to_owned()))
    }
}

/// What a container of a workspace is for: a board holds cards and their views, a wiki
/// holds pages, a diary holds the entries of its days, and a course holds chapters and
/// units. A container is of one kind from when it is made and never of another.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ContainerKind {
    /// A board: cards, their views and their comments.
    #[default]
    Board,
    /// A wiki: a tree of pages.
    Wiki,
    /// A diary: one entry a day ([`crate::DiaryEntry`]). Only a user's own space has
    /// any, and nobody is on one but its owner: it has no members to manage, no link
    /// and no copy.
    Diary,
    /// A course: an outline of chapters and units ([`crate::Unit`]).
    Course,
}

impl ContainerKind {
    /// Every kind there is.
    pub const ALL: [Self; 4] = [Self::Board, Self::Wiki, Self::Diary, Self::Course];

    /// Returns the wire and database form.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Board => "board",
            Self::Wiki => "wiki",
            Self::Diary => "diary",
            Self::Course => "course",
        }
    }
}

impl std::fmt::Display for ContainerKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when a string names no kind of board.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0:?} is no kind of board")]
pub struct ParseContainerKindError(String);

impl FromStr for ContainerKind {
    type Err = ParseContainerKindError;

    /// Reads the wire and database form ([`ContainerKind::as_str`]), exactly.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == text)
            .ok_or_else(|| ParseContainerKindError(text.to_owned()))
    }
}

/// The kinds of container something reaches: a set of [`ContainerKind`]s telling what an
/// operation is for and what a list holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContainerKinds(u8);

impl ContainerKinds {
    const BOARD: u8 = 1 << 0;
    const WIKI: u8 = 1 << 1;
    const DIARY: u8 = 1 << 2;
    const COURSE: u8 = 1 << 3;

    /// Boards alone.
    pub const BOARDS: Self = Self(Self::BOARD);
    /// Wikis alone.
    pub const WIKIS: Self = Self(Self::WIKI);
    /// Diaries alone.
    pub const DIARIES: Self = Self(Self::DIARY);
    /// Courses alone.
    pub const COURSES: Self = Self(Self::COURSE);
    /// Boards, wikis and courses: what people work on together, with members, groups and
    /// invitations. A diary is its owner's alone and has none of those.
    pub const SHARED: Self = Self(Self::BOARD | Self::WIKI | Self::COURSE);
    /// Boards, wikis and courses: what may be shared by a public link or named by a
    /// reference. A diary is never shared or referenced.
    pub const LINKED: Self = Self(Self::BOARD | Self::WIKI | Self::COURSE);
    /// Boards, wikis and courses: what may be copied whole (`duplicateContainer`).
    pub const COPIED: Self = Self(Self::BOARD | Self::WIKI | Self::COURSE);
    /// Boards, wikis and courses: open containers that whoever works in the workspace
    /// reads and searches without joining.
    pub const OPEN_TO_READ: Self = Self(Self::BOARD | Self::WIKI | Self::COURSE);
    /// Every kind: what is the same for any container (the container itself, its files,
    /// its place in the sidebar and in the trash, its archive).
    pub const EVERY: Self = Self(Self::BOARD | Self::WIKI | Self::DIARY | Self::COURSE);

    const fn bit(kind: ContainerKind) -> u8 {
        match kind {
            ContainerKind::Board => Self::BOARD,
            ContainerKind::Wiki => Self::WIKI,
            ContainerKind::Diary => Self::DIARY,
            ContainerKind::Course => Self::COURSE,
        }
    }

    /// A set of one kind.
    #[must_use]
    pub const fn of(kind: ContainerKind) -> Self {
        Self(Self::bit(kind))
    }

    /// Both sets together.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// The kinds that are in both sets.
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Whether the set holds no kind.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether a container of `kind` is among them.
    #[must_use]
    pub const fn holds(self, kind: ContainerKind) -> bool {
        (self.0 & Self::bit(kind)) != 0
    }

    /// The kinds that are among them, in the order of [`ContainerKind::ALL`].
    pub fn iter(self) -> impl Iterator<Item = ContainerKind> {
        ContainerKind::ALL
            .into_iter()
            .filter(move |kind| self.holds(*kind))
    }

    /// The one kind they are, if they are one.
    #[must_use]
    pub const fn only(self) -> Option<ContainerKind> {
        match self.0 {
            Self::BOARD => Some(ContainerKind::Board),
            Self::WIKI => Some(ContainerKind::Wiki),
            Self::DIARY => Some(ContainerKind::Diary),
            Self::COURSE => Some(ContainerKind::Course),
            _ => None,
        }
    }
}

/// The kinds that are `kind` alone: what reaches a board reaches nothing else.
impl From<ContainerKind> for ContainerKinds {
    fn from(kind: ContainerKind) -> Self {
        Self::of(kind)
    }
}

/// A role on a board, from least to most privileged.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
pub enum ContainerRole {
    /// May view the board.
    #[serde(rename = "viewer")]
    Viewer,
    /// May also comment on cards.
    #[serde(rename = "commenter")]
    Commenter,
    /// May also edit cards and properties.
    #[serde(rename = "editor")]
    Editor,
    /// May also manage the board, its members and its sharing.
    #[serde(rename = "admin")]
    Admin,
}

impl ContainerRole {
    /// Every role there is, from least to most privileged.
    pub const ALL: [Self; 4] = [Self::Viewer, Self::Commenter, Self::Editor, Self::Admin];

    /// The wire words in the order of [`Self::ALL`].
    pub const WORDS: &[&str] = &["viewer", "commenter", "editor", "admin"];

    /// Returns the wire form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Commenter => "commenter",
            Self::Editor => "editor",
            Self::Admin => "admin",
        }
    }
}

impl std::fmt::Display for ContainerRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ContainerRole {
    type Err = crate::organization::UnknownRole;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|role| role.as_str() == text)
            .ok_or_else(|| crate::organization::UnknownRole(text.to_owned()))
    }
}

/// The minimum role of a board: the role a user gets who joins the board, or is added
/// to it by somebody who does not manage its roles.
///
/// A board always has one, and it is never admin: nobody becomes an administrator of a
/// board by joining it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, utoipa::ToSchema,
)]
pub enum MinimumRole {
    /// Whoever joins may view the board.
    #[serde(rename = "viewer")]
    Viewer,
    /// Whoever joins may also comment on cards.
    #[serde(rename = "commenter")]
    Commenter,
    /// Whoever joins may also edit cards and properties. What a new board starts with.
    #[default]
    #[serde(rename = "editor")]
    Editor,
}

impl MinimumRole {
    /// Every minimum role there is.
    pub const ALL: [Self; 3] = [Self::Viewer, Self::Commenter, Self::Editor];

    /// Returns the wire and database form.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Commenter => "commenter",
            Self::Editor => "editor",
        }
    }
}

impl From<MinimumRole> for ContainerRole {
    fn from(role: MinimumRole) -> Self {
        match role {
            MinimumRole::Viewer => Self::Viewer,
            MinimumRole::Commenter => Self::Commenter,
            MinimumRole::Editor => Self::Editor,
        }
    }
}

/// Reasons a [`Container`], or a change of one, is rejected before it is stored.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ContainerError {
    /// The board names no workspace.
    #[error("empty-workspace-id")]
    EmptyWorkspaceId,
    /// A card property definition of the board is malformed.
    #[error(transparent)]
    InvalidCardProperty(#[from] CardPropertyError),
    /// A member of the board holds U+0000, which PostgreSQL cannot store in text or
    /// `jsonb`.
    #[error("the {member} of the board holds the character U+0000")]
    NulCharacter {
        /// The member, as the API names it.
        member: &'static str,
    },
    /// A new order of the card properties names a property twice.
    #[error("the order names each property of the board once: {id:?} is named twice")]
    PropertyNamedTwice {
        /// The property's identifier.
        id: String,
    },
    /// A new order of the card properties names a property the board does not have.
    #[error("the order names the properties of the board: it has no {id:?}")]
    PropertyUnknown {
        /// The identifier the order named.
        id: String,
    },
    /// A new order of the card properties leaves some of the board's properties out.
    #[error("the order names every property of the board: {named} of its {total} are named")]
    PropertiesLeftOut {
        /// How many the order names.
        named: usize,
        /// How many the board has.
        total: usize,
    },
    /// The board is stored as of another kind than it was made as.
    #[error("a {stored} does not become a {asked}: the kind of a board never changes")]
    KindChanges {
        /// The kind it is.
        stored: ContainerKind,
        /// The kind it was to become.
        asked: ContainerKind,
    },
    /// A wiki is stored as a template.
    #[error("a wiki is no template")]
    WikiIsTemplate,
    /// A wiki is stored with card properties.
    #[error("a wiki has no card properties: it holds pages and no cards")]
    WikiHasCardProperties,
    /// A diary is stored as a template.
    #[error("a diary is no template")]
    DiaryIsTemplate,
    /// A diary is stored with card properties.
    #[error("a diary has no card properties: it holds entries and no cards")]
    DiaryHasCardProperties,
    /// A diary is stored as open: there is nobody to open it to.
    #[error("a diary is private: its visibility is `private`")]
    DiaryNotPrivate,
    /// A diary is stored in a workspace that is not of a user's own space.
    #[error("a diary is of a user's own space: no other workspace has one")]
    DiaryOffOwnSpace,
    /// The space would have more diaries than it may, those in the trash not counted.
    #[error("the space would have more than {limit} diaries")]
    TooManyDiaries {
        /// The most diaries a space may have.
        limit: u32,
    },
    /// A course is stored with card properties.
    #[error("a course has no card properties: it holds units and no cards")]
    CourseHasCardProperties,
    /// `dueDatePropertyId` names no `date` card property of the board.
    #[error("dueDatePropertyId must name a date property of the board")]
    InvalidDueDateProperty,
    /// `doneWhen` names no `select`, `multiSelect` or `checkbox` property, or names an
    /// option not belonging to that property.
    #[error("doneWhen must name a select, multiSelect or checkbox property and its options")]
    InvalidDoneWhen,
}

/// A board: a set of blocks and the property definitions of its cards.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Container {
    /// The board's identifier.
    #[serde(default)]
    pub id: String,
    /// The identifier of the workspace the board belongs to.
    #[serde(default)]
    pub workspace_id: String,
    /// The identifier of the user who created the board; empty for a built-in template,
    /// which no user made.
    #[serde(default)]
    pub created_by: String,
    /// The identifier of the user who last modified the board.
    #[serde(default)]
    pub modified_by: String,
    /// Whether the board is open or private.
    pub visibility: ContainerVisibility,
    /// What the board is for: `board` for cards, `wiki` for pages, `diary` for the
    /// entries of its days, `course` for chapters and units. It is what the board was
    /// made as and never changes; a board decoded without it is a `board`. A wiki, a
    /// diary and a course are no template and have no card properties, and the
    /// operations that are about cards answer for them as for a board that is not there.
    /// A diary is private (`visibility` `private`) besides, and is only in the workspace
    /// of a user's own space ("Diaries").
    #[serde(default)]
    pub kind: ContainerKind,
    /// The role a user gets who joins the board, or is added by somebody who does not
    /// manage its roles. Never empty and never admin.
    #[serde(default)]
    pub minimum_role: MinimumRole,
    /// The board's title. No text of a board (this, `description`, `icon`, and the
    /// strings and member names of `properties` and `cardProperties`) holds U+0000:
    /// 400 naming the member.
    #[serde(default)]
    pub title: String,
    /// The board's description.
    #[serde(default)]
    pub description: String,
    /// The board's icon, usually one emoji.
    #[serde(default)]
    pub icon: String,
    /// Whether the description is shown in the interface.
    #[serde(default)]
    pub show_description: bool,
    /// Whether the board is a template.
    #[serde(default)]
    pub is_template: bool,
    /// Whether the board is one of the templates the server ships with. They are
    /// offered to every workspace and nobody can change them. The server decides this:
    /// what a client sends here is ignored.
    #[serde(default)]
    pub is_builtin: bool,
    /// The version of the built-in template this board came from.
    #[serde(default)]
    pub template_version: i64,
    /// Free-form properties of the board itself.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub properties: JsonMap,
    /// The property definitions of the board's cards, in display order.
    #[serde(default)]
    pub card_properties: Vec<CardPropertyDefinition>,
    /// Optional identifier of the `date` property used as the board's due date in
    /// `'Today'` and `due_soon` notifications. When absent, the board's first `date`
    /// property is inferred automatically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_date_property_id: Option<String>,
    /// Optional completion rule marking which cards on this board are finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_when: Option<DoneWhen>,
    /// Creation time in milliseconds since the Unix epoch.
    #[serde(default)]
    pub created_at: i64,
    /// Last modification time in milliseconds since the Unix epoch.
    #[serde(default)]
    pub updated_at: i64,
    /// Deletion time in milliseconds since the Unix epoch; absent while not deleted.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_util::positive_millis"
    )]
    #[schema(nullable = false)]
    pub deleted_at: Option<i64>,
}

impl Container {
    /// Puts the card properties into the order of their identifiers in `order`, which
    /// must name each of them exactly once.
    ///
    /// # Errors
    ///
    /// Returns [`ContainerError::PropertyNamedTwice`], [`ContainerError::PropertyUnknown`] or
    /// [`ContainerError::PropertiesLeftOut`], and changes nothing, if `order` names one twice,
    /// names one the board does not have, or leaves one out.
    pub fn reorder_card_properties(&mut self, order: &[String]) -> Result<(), ContainerError> {
        let known: HashSet<&str> = self
            .card_properties
            .iter()
            .map(|property| property.id.as_str())
            .collect();
        let mut rank = HashMap::with_capacity(order.len());
        for (at, id) in order.iter().enumerate() {
            if !known.contains(id.as_str()) {
                return Err(ContainerError::PropertyUnknown { id: id.clone() });
            }
            if rank.insert(id.as_str(), at).is_some() {
                return Err(ContainerError::PropertyNamedTwice { id: id.clone() });
            }
        }
        // Each named once and each known: the order is the board's when it names as many
        // as the board has (a stored board's identifiers are its own and distinct).
        if rank.len() != self.card_properties.len() {
            return Err(ContainerError::PropertiesLeftOut {
                named: rank.len(),
                total: self.card_properties.len(),
            });
        }
        self.card_properties
            .sort_by_key(|property| rank.get(property.id.as_str()).copied());
        Ok(())
    }

    /// The board as it goes to whoever reads it by its link, signed in or not: no
    /// `createdBy` or `modifiedBy`. The blocks go so by [`Block::hide_people`].
    ///
    /// [`Block::hide_people`]: crate::Block::hide_people
    pub fn hide_people(&mut self) {
        self.created_by.clear();
        self.modified_by.clear();
    }

    /// The properties of the board's cards whose values name users.
    #[must_use]
    pub fn person_properties(&self) -> PersonProperties {
        PersonProperties::of(&self.card_properties)
    }

    /// Returns the effective `date` property identifier used as this board's due date:
    /// `due_date_property_id` when it names a `date` property on the board, or else the
    /// first `date` property in `card_properties`.
    #[must_use]
    pub fn effective_due_date_property_id(&self) -> Option<&str> {
        if let Some(explicit) = self.due_date_property_id.as_deref()
            && self
                .card_properties
                .iter()
                .any(|p| p.id == explicit && p.property_type == PropertyType::Date)
        {
            return Some(explicit);
        }
        self.card_properties
            .iter()
            .find(|p| !p.id.is_empty() && p.property_type == PropertyType::Date)
            .map(|p| p.id.as_str())
    }

    /// Checks that the board may be stored.
    ///
    /// # Errors
    ///
    /// Returns [`ContainerError::EmptyWorkspaceId`] if the board names no workspace, or
    /// [`ContainerError::InvalidCardProperty`] if a card property definition is malformed.
    pub fn validate(&self) -> Result<(), ContainerError> {
        if self.workspace_id.is_empty() {
            return Err(ContainerError::EmptyWorkspaceId);
        }
        self.validate_card_properties()?;
        Ok(())
    }

    /// Checks what every board that is stored must be, wherever it came from: its card
    /// property definitions are well-formed, none of its texts (`title`,
    /// `description`, `icon`, and the strings and member names of `properties` and
    /// `cardProperties`) holds U+0000, a wiki and a diary are no template and have no
    /// card properties, and a diary is private.
    ///
    /// # Errors
    ///
    /// Returns [`ContainerError::InvalidCardProperty`], [`ContainerError::NulCharacter`],
    /// or what the kind refuses: [`ContainerError::WikiIsTemplate`],
    /// [`ContainerError::WikiHasCardProperties`], [`ContainerError::DiaryIsTemplate`],
    /// [`ContainerError::DiaryHasCardProperties`] or [`ContainerError::DiaryNotPrivate`].
    pub fn check_storable(&self) -> Result<(), ContainerError> {
        self.validate_card_properties()?;
        let (is_template, has_card_properties) = match self.kind {
            ContainerKind::Board => (None, None),
            ContainerKind::Wiki => (
                Some(ContainerError::WikiIsTemplate),
                Some(ContainerError::WikiHasCardProperties),
            ),
            ContainerKind::Diary => (
                Some(ContainerError::DiaryIsTemplate),
                Some(ContainerError::DiaryHasCardProperties),
            ),
            ContainerKind::Course => (None, Some(ContainerError::CourseHasCardProperties)),
        };
        if let Some(refused) = is_template.filter(|_| self.is_template) {
            return Err(refused);
        }
        if let Some(refused) = has_card_properties.filter(|_| !self.card_properties.is_empty()) {
            return Err(refused);
        }
        if self.kind == ContainerKind::Diary && self.visibility != ContainerVisibility::Private {
            return Err(ContainerError::DiaryNotPrivate);
        }
        self.check_due_and_done()?;
        let texts = [
            ("title", self.title.contains('\0')),
            ("description", self.description.contains('\0')),
            ("icon", self.icon.contains('\0')),
            (
                "properties",
                self.properties
                    .iter()
                    .any(|(name, value)| name.contains('\0') || holds_nul(value)),
            ),
            (
                "cardProperties",
                self.card_properties
                    .iter()
                    .any(CardPropertyDefinition::holds_nul),
            ),
            (
                "dueDatePropertyId",
                self.due_date_property_id
                    .as_ref()
                    .is_some_and(|id| id.contains('\0')),
            ),
            (
                "doneWhen",
                self.done_when.as_ref().is_some_and(DoneWhen::holds_nul),
            ),
        ];
        match texts.into_iter().find(|(_, holds)| *holds) {
            Some((member, _)) => Err(ContainerError::NulCharacter { member }),
            None => Ok(()),
        }
    }

    fn check_due_and_done(&self) -> Result<(), ContainerError> {
        if self.kind != ContainerKind::Board {
            if self.due_date_property_id.is_some() {
                return Err(ContainerError::InvalidDueDateProperty);
            }
            if self.done_when.is_some() {
                return Err(ContainerError::InvalidDoneWhen);
            }
            return Ok(());
        }
        if let Some(due_id) = &self.due_date_property_id
            && !self
                .card_properties
                .iter()
                .any(|p| p.id == *due_id && p.property_type == PropertyType::Date)
        {
            return Err(ContainerError::InvalidDueDateProperty);
        }
        if let Some(done) = &self.done_when {
            let Some(prop) = self
                .card_properties
                .iter()
                .find(|p| p.id == done.property_id)
            else {
                return Err(ContainerError::InvalidDoneWhen);
            };
            match prop.property_type {
                PropertyType::Checkbox => {
                    if !done.option_ids.is_empty() {
                        return Err(ContainerError::InvalidDoneWhen);
                    }
                }
                PropertyType::Select | PropertyType::MultiSelect => {
                    if done.option_ids.is_empty()
                        || !done
                            .option_ids
                            .iter()
                            .all(|id| prop.options.iter().any(|opt| opt.id == *id))
                    {
                        return Err(ContainerError::InvalidDoneWhen);
                    }
                }
                _ => return Err(ContainerError::InvalidDoneWhen),
            }
        }
        Ok(())
    }

    /// Checks the card property definitions of the board alone, for where a board is
    /// stored that came no way that checks the rest.
    ///
    /// # Errors
    ///
    /// Returns the error of [`validate_card_property_definitions`].
    pub fn validate_card_properties(&self) -> Result<(), CardPropertyError> {
        validate_card_property_definitions(&self.card_properties)
    }
}

/// A partial update of a `Container`. Absent members leave the board unchanged.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ContainerPatch {
    /// The new board visibility.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<ContainerVisibility>,
    /// The new minimum role.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_role: Option<MinimumRole>,
    /// The new title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The new description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The new icon.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Whether the description is shown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_description: Option<bool>,
    /// Board properties to set.
    #[schema(value_type = Object)]
    pub updated_properties: JsonMap,
    /// Keys of board properties to remove.
    pub deleted_properties: Vec<String>,
    /// Card property definitions to add or replace, matched by their `id`.
    pub updated_card_properties: Vec<CardPropertyDefinition>,
    /// Identifiers of card property definitions to remove.
    pub deleted_card_properties: Vec<String>,
    /// Optional change to `dueDatePropertyId`: omitted keeps it, `null` or `""` clears it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_util::nullable_patch"
    )]
    #[schema(value_type = Option<String>)]
    pub due_date_property_id: Option<Option<String>>,
    /// Optional change to `doneWhen`: omitted keeps it, `null` clears it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_util::nullable_patch"
    )]
    #[schema(value_type = Option<DoneWhen>)]
    pub done_when: Option<Option<DoneWhen>>,
}

impl ContainerPatch {
    /// Checks the card property definitions the patch sets. Applying the patch would
    /// drop a definition without an `id` silently; a client that sends one is told.
    ///
    /// # Errors
    ///
    /// Returns the error of [`validate_card_property_definitions`].
    pub fn validate(&self) -> Result<(), CardPropertyError> {
        validate_card_property_definitions(&self.updated_card_properties)
    }

    /// Applies the patch to `container`.
    ///
    /// Card properties keep their existing order; replaced definitions stay in place and
    /// new ones are appended. Deletions are applied before updates, so a definition listed
    /// in both is kept with its updated value, and then appears twice: once at its old
    /// position and once at the end. Whenever the patch touches card properties, existing
    /// definitions with an empty `id` are dropped.
    pub fn apply(&self, container: &mut Container) {
        if let Some(visibility) = self.visibility {
            container.visibility = visibility;
        }
        if let Some(title) = &self.title {
            container.title.clone_from(title);
        }
        if let Some(minimum_role) = self.minimum_role {
            container.minimum_role = minimum_role;
        }
        if let Some(description) = &self.description {
            container.description.clone_from(description);
        }
        if let Some(icon) = &self.icon {
            container.icon.clone_from(icon);
        }
        if let Some(show_description) = self.show_description {
            container.show_description = show_description;
        }
        for (key, value) in &self.updated_properties {
            container.properties.insert(key.clone(), value.clone());
        }
        for key in &self.deleted_properties {
            container.properties.remove(key);
        }
        if !self.updated_card_properties.is_empty() || !self.deleted_card_properties.is_empty() {
            self.apply_card_properties(container);
        }
        if let Some(next_due) = &self.due_date_property_id {
            container.due_date_property_id = next_due.clone().filter(|s| !s.is_empty());
        }
        if let Some(next_done) = &self.done_when {
            container.done_when.clone_from(next_done);
        }
        reconcile_due_and_done(container);
    }

    fn apply_card_properties(&self, container: &mut Container) {
        let mut order: Vec<String> = Vec::new();
        let mut by_id: HashMap<String, CardPropertyDefinition> = HashMap::new();
        for property in std::mem::take(&mut container.card_properties) {
            if !property.id.is_empty() {
                order.push(property.id.clone());
                by_id.insert(property.id.clone(), property);
            }
        }
        for id in &self.deleted_card_properties {
            by_id.remove(id);
        }
        for property in &self.updated_card_properties {
            if !property.id.is_empty() {
                if !by_id.contains_key(&property.id) {
                    order.push(property.id.clone());
                }
                by_id.insert(property.id.clone(), property.clone());
            }
        }
        // An identifier can be in `order` twice if the patch deletes it and adds it back.
        // Emit each definition once, at its first position, to avoid duplicate property IDs.
        let mut emitted = HashSet::new();
        container.card_properties = order
            .iter()
            .filter(|id| emitted.insert(id.as_str()))
            .filter_map(|id| by_id.get(id).cloned())
            .collect();
    }
}

fn reconcile_due_and_done(container: &mut Container) {
    if let Some(due_id) = &container.due_date_property_id
        && !container
            .card_properties
            .iter()
            .any(|p| p.id == *due_id && p.property_type == PropertyType::Date)
    {
        container.due_date_property_id = None;
    }
    if let Some(done) = &mut container.done_when {
        let Some(prop) = container
            .card_properties
            .iter()
            .find(|p| p.id == done.property_id)
        else {
            container.done_when = None;
            return;
        };
        match prop.property_type {
            PropertyType::Checkbox => done.option_ids.clear(),
            PropertyType::Select | PropertyType::MultiSelect => {
                done.option_ids
                    .retain(|id| prop.options.iter().any(|opt| opt.id == *id));
                if done.option_ids.is_empty() {
                    container.done_when = None;
                }
            }
            _ => container.done_when = None,
        }
    }
}

/// A user's membership of a board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContainerMember {
    /// The identifier of the board.
    #[serde(default)]
    pub container_id: String,
    /// The identifier of the user.
    #[serde(default)]
    pub user_id: String,
    /// The user's role on the board.
    pub role: ContainerRole,
    /// The membership has no directly stored role record.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub synthetic: bool,
    /// The highest role inherited from teams; the direct `role` remains unchanged.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_util::non_null"
    )]
    #[schema(nullable = false)]
    pub inherited_role: Option<MinimumRole>,
}

impl ContainerMember {
    /// A direct membership of `user_id` on `container_id` with `role`.
    #[must_use]
    pub fn new(
        container_id: impl Into<String>,
        user_id: impl Into<String>,
        role: ContainerRole,
    ) -> Self {
        Self {
            container_id: container_id.into(),
            user_id: user_id.into(),
            role,
            synthetic: false,
            inherited_role: None,
        }
    }

    /// The effective role the membership grants: the higher of its `role` and any
    /// `inherited_role`.
    #[must_use]
    pub fn effective_role(&self) -> ContainerRole {
        match self.inherited_role {
            Some(inherited) => self.role.max(ContainerRole::from(inherited)),
            None => self.role,
        }
    }

    /// The membership with `role` as its direct role.
    #[must_use]
    pub fn with_role(self, role: ContainerRole) -> Self {
        Self { role, ..self }
    }
}

/// A `(container_id, user_id)` pair naming a person on a board without a role,
/// carried when a membership ends or a group's board reach is rechecked.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberRef {
    /// The identifier of the board.
    pub container_id: String,
    /// The identifier of the user.
    pub user_id: String,
}

impl MemberRef {
    /// Names `user_id` on `container_id`.
    #[must_use]
    pub fn new(container_id: impl Into<String>, user_id: impl Into<String>) -> Self {
        Self {
            container_id: container_id.into(),
            user_id: user_id.into(),
        }
    }
}

/// A change to a member's role on a board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContainerMemberPatch {
    /// The role the member is to have on the board.
    pub role: ContainerRole,
}

/// The board member that a board search matches against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContainerSearchField {
    /// Match the search term against board titles.
    Title,
    /// Match the search term against the names of card properties.
    PropertyName,
}

/// Error returned when a string names no board search field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("invalid board search field")]
pub struct ParseContainerSearchFieldError;

impl FromStr for ContainerSearchField {
    type Err = ParseContainerSearchFieldError;

    fn from_str(field: &str) -> Result<Self, Self::Err> {
        match field {
            "title" => Ok(Self::Title),
            "property_name" => Ok(Self::PropertyName),
            _ => Err(ParseContainerSearchFieldError),
        }
    }
}
