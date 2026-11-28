//! What the model refuses of a block, a board, a relation or a card a client sent, as the
//! problems the API answers with.

use fvoci_model::{
    BlockError, CardError, ContainerError, RELATION_LIMIT, RelationError, RelationProblem,
};

use super::{AppError, Reason, Subject};

impl From<BlockError> for AppError {
    fn from(error: BlockError) -> Self {
        match error {
            BlockError::TitleSizeLimitExceeded
            | BlockError::FieldsSizeLimitExceeded
            | BlockError::InvalidCardProperties(_) => Self::bad_request(error),
            BlockError::BodyNotAllowed
            | BlockError::BodyFileNotOfContainer
            | BlockError::FileBlockInComment
            | BlockError::BoardViewNotAllowed
            | BlockError::BodyTooLarge => Self::bad_request(error).with_field("body"),
            BlockError::NulInTitle => Self::bad_request(error).with_field("title"),
            BlockError::InvalidCardField { .. }
            | BlockError::InvalidViewField { .. }
            | BlockError::InvalidPageField { .. }
            | BlockError::InvalidEntryField { .. }
            | BlockError::InvalidUnitField { .. }
            | BlockError::EntryDateChanges
            | BlockError::TemplateCardHasChildren => Self::bad_request(error).with_field("fields"),
            BlockError::DayTaken { .. } => Self::conflict(error).with_reason(Reason::DayTaken),
            BlockError::TooManyEntries { limit } => Self::bad_request(error)
                .with_reason(Reason::TooManyEntries)
                .with_limit(u64::from(limit)),
            BlockError::TooManyUnits { limit } => Self::bad_request(error)
                .with_reason(Reason::TooManyUnits)
                .with_limit(u64::from(limit)),
            BlockError::TooManyChapters { limit } => Self::bad_request(error)
                .with_reason(Reason::TooManyChapters)
                .with_limit(u64::from(limit)),
            BlockError::AfterNotSibling => Self::bad_request(error).with_field("after"),
            BlockError::PageTooDeep { limit } => Self::bad_request(error)
                .with_reason(Reason::PageTooDeep)
                .with_limit(u64::from(limit))
                .with_field("parentId"),
            BlockError::CardTooDeep { limit } => Self::bad_request(error)
                .with_reason(Reason::CardTooDeep)
                .with_limit(u64::from(limit))
                .with_field("parentId"),
            BlockError::RecurrenceNeedsBoardRules => Self::bad_request(error)
                .with_reason(Reason::RecurrenceNeedsBoardRules)
                .with_field("recurrence"),
            BlockError::TooManyPages { limit } => Self::bad_request(error)
                .with_reason(Reason::TooManyPages)
                .with_limit(u64::from(limit)),
            BlockError::InvalidRelation(error) => Self::from(error),
            BlockError::TypeNotCurrent
            | BlockError::TypeChanges
            | BlockError::PageByBlocks
            | BlockError::PageOffWiki
            | BlockError::NotPageOnWiki
            | BlockError::EntryByBlocks
            | BlockError::EntryOffDiary
            | BlockError::NotEntryOnDiary
            | BlockError::UnitByBlocks
            | BlockError::UnitOffCourse
            | BlockError::NotUnitOnCourse => Self::bad_request(error).with_field("type"),
            BlockError::ParentNotOnContainer
            | BlockError::ParentBelowBlock
            | BlockError::CommentNotOnCardOrPage
            | BlockError::ParentIsComment
            | BlockError::ViewBelowBlock
            | BlockError::ParentIsView
            | BlockError::AttachmentNotOnCard
            | BlockError::ParentIsAttachment
            | BlockError::PageNotBelowPage
            | BlockError::EntryBelowBlock
            | BlockError::UnitParentNotChapter
            | BlockError::ChapterBelowBlock
            | BlockError::CardNotBelowCard
            | BlockError::CardBelowTemplate
            | BlockError::TemplateCardBelowBlock => Self::bad_request(error).with_field("parentId"),
            BlockError::EmptyContainerId => Self::from(anyhow::Error::from(error)),
        }
    }
}

impl From<RelationError> for AppError {
    /// A bad request on `fields` (where a card's property values are), with the reason
    /// of the problem: the words name the property, and no card.
    fn from(error: RelationError) -> Self {
        let reason = match error.problem {
            RelationProblem::NotAList => Reason::RelationNotAList,
            RelationProblem::ToItself => Reason::RelationToItself,
            RelationProblem::Repeated => Reason::RelationRepeated,
            RelationProblem::TooMany => Reason::RelationTooMany,
            RelationProblem::NotACard => Reason::RelationNotACard,
            RelationProblem::CardNotFound => Reason::RelationCardNotFound,
            RelationProblem::Cycle => Reason::RelationCycle,
        };
        let refused = Self::bad_request(&error).with_reason(reason);
        let refused = if error.problem == RelationProblem::TooMany {
            refused.with_limit(u64::try_from(RELATION_LIMIT).unwrap_or(u64::MAX))
        } else {
            refused
        };
        refused.with_field("fields")
    }
}

impl From<CardError> for AppError {
    fn from(error: CardError) -> Self {
        match error {
            // What a client sent.
            CardError::BoardIdMismatch | CardError::Invalid(_) => Self::bad_request(error),
            CardError::ReadAtMissing => Self::bad_request(error).with_field("readAt"),
            // What is stored cannot be read as a card.
            CardError::NotCardBlock | CardError::InvalidFieldType { .. } => {
                Self::from(anyhow::Error::from(error))
            }
        }
    }
}

impl AppError {
    /// A bad request for what a board of its kind is not, at the member of the board
    /// that says otherwise: a template or card properties of a wiki or a diary, a diary
    /// that is not private, and any other at its kind. One diary more than a space has
    /// is refused with its reason and the limit, at no member.
    pub(super) fn not_of_its_kind(refused: ContainerError) -> Self {
        if let ContainerError::TooManyDiaries { limit } = refused {
            return Self::bad_request(refused)
                .with_reason(Reason::TooManyDiaries)
                .with_limit(u64::from(limit));
        }
        let member = match refused {
            ContainerError::WikiIsTemplate | ContainerError::DiaryIsTemplate => "isTemplate",
            ContainerError::WikiHasCardProperties
            | ContainerError::DiaryHasCardProperties
            | ContainerError::CourseHasCardProperties => "cardProperties",
            ContainerError::DiaryNotPrivate => "type",
            _ => "kind",
        };
        let mut error = Self::bad_request(refused).with_field(member);
        error.subject = Some(Subject::Board);
        error
    }
}
