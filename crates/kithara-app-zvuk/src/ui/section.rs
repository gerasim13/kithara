use kithara_app_library::worded;
use kithara_ui::{error::UiDocError, render::ReadValue, text::TextDoc};

use crate::account::{AccountError, Command, Row, State};

/// The account row as the settings section reads it.
pub(super) struct Section {
    pub(super) row: Row,
    faults: FaultWords,
}

/// The catalog's words for every account fault, taken when the source is
/// built.
struct FaultWords {
    session: String,
    browser: String,
    confirmation: String,
    store: String,
    rejected: String,
}

impl FaultWords {
    fn new(text: &TextDoc) -> Result<Self, UiDocError> {
        let word = |key| worded(text, key, "source.account_fault");
        Ok(Self {
            session: word("zvuk.account.fault.session")?,
            browser: word("zvuk.account.fault.browser")?,
            confirmation: word("zvuk.account.fault.confirmation")?,
            store: word("zvuk.account.fault.store")?,
            rejected: word("zvuk.account.fault.rejected")?,
        })
    }

    fn of(&self, fault: AccountError) -> &str {
        match fault {
            AccountError::Session => &self.session,
            AccountError::Browser => &self.browser,
            AccountError::Confirmation => &self.confirmation,
            AccountError::Store => &self.store,
            AccountError::Rejected => &self.rejected,
        }
    }
}

impl Section {
    pub(super) fn new(text: &TextDoc, row: Row) -> Result<Self, UiDocError> {
        Ok(Self {
            row,
            faults: FaultWords::new(text)?,
        })
    }

    pub(super) fn read(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        let state = &self.row.state;
        let label = match state {
            State::Connected { label } => label.as_deref(),
            State::SignedOut | State::Waiting => None,
        };
        let fault = self.row.fault.map_or("", |fault| self.faults.of(fault));
        Some(match endpoint {
            "account_label" => ReadValue::Text(label.unwrap_or_default()),
            "account_label_hidden" => ReadValue::Bool(label.is_none()),
            "account_fault" => ReadValue::Text(fault),
            "account_fault_hidden" => ReadValue::Bool(self.row.fault.is_none()),
            "account_connect_hidden" => ReadValue::Bool(*state != State::SignedOut),
            "account_awaiting_hidden" => ReadValue::Bool(*state != State::Waiting),
            "account_disconnect_hidden" => {
                ReadValue::Bool(!matches!(state, State::Connected { .. }))
            }
            _ => return None,
        })
    }
}

/// The account command an `account_*` write asks for.
pub(super) fn command(endpoint: &str) -> Option<Command> {
    match endpoint {
        "account_connect" => Some(Command::Connect),
        "account_cancel" => Some(Command::Cancel),
        "account_disconnect" => Some(Command::Disconnect),
        _ => None,
    }
}
