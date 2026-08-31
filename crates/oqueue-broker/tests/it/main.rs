//! `oqueue-broker`'s integration tests — T1: a runtime and a duplex pipe,
//! no socket, no container.

mod budget;
mod connection;
mod corpus;
mod crash_points;
mod faults;
mod invariants;
mod matrix;
mod reads;
mod reap;
mod roundtrip;
mod support;
mod virtual_time;
