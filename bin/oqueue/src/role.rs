#![allow(clippy::expect_used)]
#![allow(clippy::redundant_pub_crate)]

use oqueue_core::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProcessRole {
    Coordinator,
    DataPlane,
    Combined,
}

impl ProcessRole {
    pub(crate) const fn broker_role(self) -> oqueue_broker::NodeRole {
        match self {
            Self::Coordinator => oqueue_broker::NodeRole::Coordinator,
            Self::DataPlane => oqueue_broker::NodeRole::DataPlane,
            Self::Combined => oqueue_broker::NodeRole::Combined,
        }
    }
}

fn from_selection(selection: Result<String, std::env::VarError>) -> Result<ProcessRole, Error> {
    match selection {
        Ok(value) => match value.as_str() {
            "coordinator" => Ok(ProcessRole::Coordinator),
            "data-plane" => Ok(ProcessRole::DataPlane),
            "combined" => Ok(ProcessRole::Combined),
            _ => Err(Error::Permanent),
        },
        Err(std::env::VarError::NotPresent) => Ok(ProcessRole::Combined),
        Err(std::env::VarError::NotUnicode(_)) => Err(Error::Permanent),
    }
}

pub(crate) fn selected() -> ProcessRole {
    match from_selection(std::env::var("OQUEUE_ROLE")) {
        Ok(role) => role,
        Err(error) => {
            eprintln!(
                "oqueue: OQUEUE_ROLE must be one of coordinator, data-plane, or combined: {error}"
            );
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ProcessRole, from_selection};

    #[test]
    fn role_selection_is_explicit_and_defaults_to_combined() {
        assert_eq!(
            from_selection(Err(std::env::VarError::NotPresent)).expect("unset role"),
            ProcessRole::Combined
        );
        assert_eq!(
            from_selection(Ok("coordinator".to_owned())).expect("coordinator role"),
            ProcessRole::Coordinator
        );
        assert_eq!(
            from_selection(Ok("data-plane".to_owned())).expect("data-plane role"),
            ProcessRole::DataPlane
        );
        assert_eq!(
            from_selection(Ok("combined".to_owned())).expect("combined role"),
            ProcessRole::Combined
        );
        assert!(from_selection(Ok("broker".to_owned())).is_err());
    }
}
