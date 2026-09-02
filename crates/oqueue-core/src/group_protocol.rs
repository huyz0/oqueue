//! Group-leader election and protocol-name intersection (`M4.6`, FR-22).
//!
//! ⚠️ Doc 02 §3.1: "The coordinator designates a group leader (conventionally
//! the first member to join) and picks the one assignment protocol name
//! every current member advertises support for." This module is that
//! decision made a pure function over a join-order snapshot — no I/O, no
//! group state, no wire bytes. `M4.7`'s handler is the only intended caller,
//! and deciding *when* to call this (the join barrier closing) is that
//! task's own job, not this module's.

/// One member's own candidacy: the protocol names it advertises, in the
/// member's own preference order.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    /// The member's own id.
    pub member_id: &'a str,
    /// Every protocol name this member supports, most preferred first.
    pub protocols: &'a [&'a str],
}

/// The result of electing a leader and negotiating a protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Elected<'a> {
    /// The elected leader's own member id.
    pub leader: &'a str,
    /// The one protocol name every member advertised.
    pub protocol_name: &'a str,
}

/// Elects a leader and negotiates the one protocol every member supports.
///
/// The leader is `members`' own first entry — join order, doc 02 §3.1's own
/// "conventionally the first member to join"; the caller supplies that
/// order, this function does not sort or otherwise infer it.
///
/// The protocol is chosen from the intersection of every member's own
/// advertised protocol names — the set no member fails to advertise. Each
/// member then "votes" for the first name in its own preference list that
/// survives the intersection, and the name with the most votes wins; a tie
/// is broken by the intersection's own order, which is the leader's own
/// preference order (the leader is the intersection's starting point,
/// below).
///
/// Returns `None` if `members` is empty, or if the intersection is empty —
/// no protocol name every member advertised. Both are caught here, before a
/// leader is ever handed to a caller with no protocol for that leader to
/// run — `M4.6`'s own acceptance criterion. Mapping that to a wire error
/// code (`INCONSISTENT_GROUP_PROTOCOL`) is `M4.7`'s handler's own job: this
/// function's callers are not exclusively wire handlers, so it stays a
/// plain `Option` rather than reaching into `crate::Error` for a variant
/// only one caller would ever produce.
#[must_use]
pub fn elect<'a>(members: &[Candidate<'a>]) -> Option<Elected<'a>> {
    let leader = members.first()?;

    let mut candidates: Vec<&'a str> = leader.protocols.to_vec();
    for member in &members[1..] {
        candidates.retain(|name| member.protocols.contains(name));
    }
    if candidates.is_empty() {
        return None;
    }

    let mut votes: Vec<(&'a str, u32)> = candidates.iter().map(|&name| (name, 0)).collect();
    for member in members {
        let Some(&voted_for) = member
            .protocols
            .iter()
            .find(|name| candidates.contains(name))
        else {
            continue;
        };
        if let Some(entry) = votes.iter_mut().find(|(name, _)| *name == voted_for) {
            entry.1 += 1;
        }
    }

    // Ties break toward the intersection's own (== the leader's own
    // preference) order: scanning forward and only replacing on a strictly
    // greater count keeps the first-seen maximum.
    let mut winner = votes[0];
    for &(name, count) in &votes[1..] {
        if count > winner.1 {
            winner = (name, count);
        }
    }

    Some(Elected {
        leader: leader.member_id,
        protocol_name: winner.0,
    })
}

#[cfg(test)]
mod tests {
    use super::{Candidate, Elected, elect};
    use proptest::prelude::*;

    #[test]
    fn no_members_elects_nothing() {
        assert_eq!(elect(&[]), None);
    }

    #[test]
    fn a_single_member_elects_itself_leader_on_its_own_first_protocol() {
        let protocols = ["range", "cooperative-sticky"];
        let candidates = [Candidate {
            member_id: "m1",
            protocols: &protocols,
        }];
        assert_eq!(
            elect(&candidates),
            Some(Elected {
                leader: "m1",
                protocol_name: "range",
            })
        );
    }

    #[test]
    fn the_first_member_is_the_leader_even_when_it_loses_the_protocol_vote() {
        // m1 votes "sticky" (loses 1-2); m2 and m3 both vote "range". m1 is
        // still leader -- leadership is join order, not the vote's winner.
        let p1 = ["sticky", "range"];
        let p2 = ["range", "sticky"];
        let p3 = ["range", "sticky"];
        let candidates = [
            Candidate {
                member_id: "m1",
                protocols: &p1,
            },
            Candidate {
                member_id: "m2",
                protocols: &p2,
            },
            Candidate {
                member_id: "m3",
                protocols: &p3,
            },
        ];
        assert_eq!(
            elect(&candidates),
            Some(Elected {
                leader: "m1",
                protocol_name: "range",
            })
        );
    }

    #[test]
    fn a_true_vote_tie_breaks_toward_the_leaders_own_earlier_preference() {
        // Both protocols survive the intersection; m1 (the leader) votes
        // "range" first, m2 votes "sticky" first -- a genuine 1-1 tie.
        // The tie-break is the intersection's own order, which is the
        // leader's own preference order: "range" before "sticky".
        let p1 = ["range", "sticky"];
        let p2 = ["sticky", "range"];
        let candidates = [
            Candidate {
                member_id: "m1",
                protocols: &p1,
            },
            Candidate {
                member_id: "m2",
                protocols: &p2,
            },
        ];
        assert_eq!(
            elect(&candidates),
            Some(Elected {
                leader: "m1",
                protocol_name: "range",
            })
        );
    }

    #[test]
    fn no_common_protocol_elects_no_leader() {
        let p1 = ["range"];
        let p2 = ["sticky"];
        let candidates = [
            Candidate {
                member_id: "m1",
                protocols: &p1,
            },
            Candidate {
                member_id: "m2",
                protocols: &p2,
            },
        ];
        assert_eq!(elect(&candidates), None);
    }

    #[test]
    fn a_shared_protocol_absent_from_the_leaders_own_list_is_never_chosen() {
        // The intersection can only ever contain names the leader itself
        // advertises -- "sticky" is common to m2 and m3 but m1 never lists
        // it, so it can never win regardless of vote count.
        let p1 = ["range"];
        let p2 = ["sticky", "range"];
        let p3 = ["sticky", "range"];
        let candidates = [
            Candidate {
                member_id: "m1",
                protocols: &p1,
            },
            Candidate {
                member_id: "m2",
                protocols: &p2,
            },
            Candidate {
                member_id: "m3",
                protocols: &p3,
            },
        ];
        assert_eq!(
            elect(&candidates),
            Some(Elected {
                leader: "m1",
                protocol_name: "range",
            })
        );
    }

    fn protocol_name() -> impl Strategy<Value = String> {
        "[a-c]{1,3}"
    }

    fn member() -> impl Strategy<Value = (String, Vec<String>)> {
        ("[a-z]{1,6}", prop::collection::vec(protocol_name(), 1..5))
    }

    fn as_candidates(members: &[(String, Vec<String>)]) -> Vec<(String, Vec<&str>)> {
        members
            .iter()
            .map(|(id, protocols)| (id.clone(), protocols.iter().map(String::as_str).collect()))
            .collect()
    }

    proptest! {
        /// `M4.6`'s own acceptance criterion, half one: for any non-empty
        /// set of members each advertising a non-empty protocol-name list,
        /// a chosen protocol (if any) is in every member's own list.
        #[test]
        fn the_chosen_protocol_is_in_every_members_own_list(
            members in prop::collection::vec(member(), 1..8),
        ) {
            let owned = as_candidates(&members);
            let candidates: Vec<Candidate<'_>> = owned
                .iter()
                .map(|(id, protocols)| Candidate { member_id: id.as_str(), protocols: protocols.as_slice() })
                .collect();

            if let Some(elected) = elect(&candidates) {
                for (_, protocols) in &members {
                    prop_assert!(protocols.iter().any(|p| p.as_str() == elected.protocol_name));
                }
            }
        }

        /// Acceptance criterion, half two: an empty intersection never
        /// elects a leader. Forced by prefixing each member's own protocol
        /// names with its own index, which makes every member's own set
        /// disjoint from every other's.
        #[test]
        fn an_empty_intersection_elects_no_leader(
            members in prop::collection::vec(member(), 2..8),
        ) {
            let disjoint: Vec<(String, Vec<String>)> = members
                .into_iter()
                .enumerate()
                .map(|(i, (id, protocols))| {
                    (
                        id,
                        protocols.into_iter().map(|p| format!("{i}-{p}")).collect(),
                    )
                })
                .collect();
            let owned = as_candidates(&disjoint);
            let candidates: Vec<Candidate<'_>> = owned
                .iter()
                .map(|(id, protocols)| Candidate { member_id: id.as_str(), protocols: protocols.as_slice() })
                .collect();

            prop_assert_eq!(elect(&candidates), None);
        }
    }
}
