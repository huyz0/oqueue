//! Which member leads a round.
//!
//! ⚠️ **Its own file because leadership is a separate property from
//! liveness**, and one a real client made expensive: the leader runs the
//! assignor, so choosing the wrong one starves a consumer even when every
//! member is alive, the roster is right, and the round closes on time.

#![allow(clippy::expect_used)]

use super::super::super::super::{VERSION, join};
use super::super::super::{request_body_offering, round_of};
use crate::testing::fixture;

/// Every id rejoining the open round at once, each on its own task.
fn rejoin_all(
    cluster: &std::sync::Arc<crate::cluster::Cluster>,
    ids: &[String],
) -> Vec<tokio::task::JoinHandle<crate::join_group::tests::KpResponse>> {
    ids.iter()
        .map(|id| {
            let cluster = std::sync::Arc::clone(cluster);
            let body =
                request_body_offering("orders", id, &[("cooperative-sticky", b"owns:p0")], VERSION);
            tokio::spawn(async move { join(&cluster, body, VERSION).await })
        })
        .collect()
}

/// ⚠️ **A member joining a running group must not become its leader.**
/// `elect` takes `members.first()`, and a round's members are in the order
/// they *enrolled* — which puts the joiner that opened the round at the
/// front, and that joiner is precisely the new consumer. So every time
/// anybody joined a working group, the newest member led it.
///
/// ⚠️ **Kafka never has this problem**: its group membership is join-ordered
/// and the leader is the longest-standing member, which is what
/// `finalize_close` restores by ordering each round's members by their
/// position in the previous roster. A sticky assignor is entitled to assume
/// a stable leader, and a leader that changes on every rebalance is a
/// divergence worth closing whatever else it causes.
///
/// ⚠️ **It is not, however, what starved the newcomer in `M4.17`.** That was
/// `sync_group`'s generation-keyed assignment barrier, and review verified
/// the client harness passes with this ordering removed. The ordering is
/// pinned by this test and by
/// `round::tests::a_closed_rounds_own_protocol_type_is_the_leaders_own`,
/// which removing the sort also fails — but by no client-level evidence,
/// since no harness leg shows a symptom it alone produces. Recorded because
/// the first version of this comment claimed the opposite.
#[tokio::test(start_paused = true)]
async fn a_newcomer_does_not_take_the_lead_from_an_incumbent() {
    let fixture = std::sync::Arc::new(fixture(&[]).await);
    let cluster = std::sync::Arc::clone(&fixture.cluster);

    let first = round_of(&cluster, &[("", b"owns:"), ("", b"owns:")]).await;
    let ids: Vec<String> = first.iter().map(|r| r.member_id.to_string()).collect();
    let leader_before = first[0].leader.to_string();
    assert!(
        ids.contains(&leader_before),
        "the first round's leader is one of its own members"
    );

    // The newcomer joins first, so it enrols first and heads `round.members`
    // — the exact ordering that used to hand it the lead.
    let newcomer = {
        let cluster = std::sync::Arc::clone(&cluster);
        let body =
            request_body_offering("orders", "", &[("cooperative-sticky", b"owns:")], VERSION);
        tokio::spawn(async move { join(&cluster, body, VERSION).await })
    };
    tokio::task::yield_now().await;

    let rejoins = rejoin_all(&cluster, &ids);

    tokio::time::advance(std::time::Duration::from_secs(6)).await;
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }

    let newcomer = newcomer.await.expect("joins");
    assert_eq!(newcomer.error_code, 0, "the newcomer joins");
    assert_eq!(
        newcomer.leader.as_str(),
        leader_before,
        "the incumbent leader keeps the lead — a newcomer leading the round \
         that admits it is a leader with no memory of the previous \
         assignment, which is how an added consumer ends up with nothing"
    );
    assert_ne!(
        newcomer.member_id.as_str(),
        newcomer.leader.as_str(),
        "and the newcomer is specifically not it, despite enrolling first"
    );
    for rejoin in rejoins {
        let response = rejoin.await.expect("joins");
        assert_eq!(response.error_code, 0, "the incumbents rejoin");
        assert_eq!(
            response.leader.as_str(),
            leader_before,
            "every member is told the same leader"
        );
    }
}
