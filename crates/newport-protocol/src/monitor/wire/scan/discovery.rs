//! Allocation bounds for discovery snapshots before generated decoding.
use super::{Error, Invalid, Kind, Rule, fields, walk};
use crate::monitor::ports::{MAX_ARGUMENTS, MAX_FIELD, MAX_LISTENERS, discovery as contract};

pub(crate) fn check(body: &[u8]) -> Result<(), Error> {
    measure(body).map(|_| ())
}

pub(crate) fn measure(body: &[u8]) -> Result<usize, Error> {
    let mut fields = 0;
    walk(body, SNAPSHOT, &mut fields)?;
    records(body)?;
    Ok(fields)
}

/// Combined allocation counts for a shape-checked snapshot. This second,
/// allocation-free pass also bounds details and holder references that are
/// distributed among different parent records.
pub(crate) fn records(mut body: &[u8]) -> Result<[usize; 5], Error> {
    let mut counts = [0usize; 5];
    let mut holders = 0usize;
    while !body.is_empty() {
        let field = fields::field(&mut body)?;
        match field.number {
            5 => {
                counts[0] += 1;
                holders += count(field.bytes()?, 10)?;
            }
            6 => {
                counts[1] += 1;
                counts[4] += count(field.bytes()?, 7)?;
            }
            7 => counts[2] += 1,
            8 => counts[3] += 1,
            _ => {}
        }
        if counts[4] > contract::MAX_DETAILS || holders > contract::MAX_HOLDER_REFERENCES {
            return Err(Invalid("encoded discovery allocation count").into());
        }
    }
    Ok(counts)
}

fn count(mut body: &[u8], tag: u32) -> Result<usize, Error> {
    let mut count = 0;
    while !body.is_empty() {
        if fields::field(&mut body)?.number == tag {
            count += 1;
        }
    }
    Ok(count)
}

pub(crate) fn measure_report(body: &[u8]) -> Result<usize, Error> {
    let mut fields = 0;
    walk(body, REPORT, &mut fields)?;
    Ok(fields)
}

pub(crate) fn check_helper_request(body: &[u8], admitted: bool) -> Result<(), Error> {
    walk(
        body,
        if admitted {
            HELPER_REQUEST
        } else {
            HELPER_REQUEST_HEADER
        },
        &mut 0,
    )
}

pub(crate) fn check_helper_reply(body: &[u8], admitted: bool) -> Result<(), Error> {
    walk(
        body,
        if admitted {
            HELPER_REPLY
        } else {
            HELPER_HEADER
        },
        &mut 0,
    )
}

schema!(HELPER_REQUEST {
    1 => (1, false, Kind::Text(64)), 2 => (1, false, Kind::Scalar),
    3 => (1, false, Kind::Discovery),
});
schema!(HELPER_REQUEST_HEADER {
    1 => (1, false, Kind::Text(64)), 2 => (1, false, Kind::Scalar),
    3 => (1, false, Kind::Text(crate::monitor::SERVER_LIMITS.message)),
});
// Admission reads only the envelope; an incompatible payload stays opaque.
schema!(HELPER_HEADER {
    1 => (1, false, Kind::Text(64)), 2 => (1, false, Kind::Scalar),
    3 => (1, true, Kind::Text(crate::monitor::SERVER_LIMITS.message)),
    4 => (1, true, Kind::Text(crate::monitor::SERVER_LIMITS.message)),
});
schema!(HELPER_REPLY {
    1 => (1, false, Kind::Text(64)), 2 => (1, false, Kind::Scalar),
    3 => (1, true, Kind::Discovery),
    4 => (1, true, Kind::Message(super::ERROR)),
});

schema!(EMPTY {});
schema!(NAMESPACE {
    1 => (1, false, Kind::Scalar), 2 => (1, false, Kind::Scalar),
});
schema!(SCOPE {
    1 => (1, false, Kind::Message(NAMESPACE)),
    2 => (1, false, Kind::Message(NAMESPACE)),
    3 => (1, false, Kind::Text(36)),
});
schema!(ENDPOINT {
    1 => (1, false, Kind::Text(16)),
    2 => (1, false, Kind::Integer32), 3 => (1, false, Kind::Integer32),
});
schema!(CGROUP {
    1 => (1, false, Kind::Scalar), 2 => (1, false, Kind::Text(MAX_FIELD)),
});
schema!(ASSOCIATION {
    1 => (1, false, Kind::Integer32),
    2 => (1, true, Kind::Message(EMPTY)), 3 => (1, true, Kind::Integer32),
});
schema!(SOCKET {
    1 => (1, false, Kind::Integer32),
    2 => (1, false, Kind::Message(ENDPOINT)),
    3 => (1, false, Kind::Integer32), 4 => (1, false, Kind::Scalar),
    5 => (1, false, Kind::Scalar), 6 => (1, false, Kind::Integer32),
    7 => (1, false, Kind::Text(MAX_FIELD)), 8 => (1, false, Kind::Bool),
    9 => (1, false, Kind::Message(CGROUP)),
    10 => (contract::MAX_HOLDERS, false, Kind::Integer32),
    11 => (1, false, Kind::Integer32), 12 => (1, false, Kind::Bool),
    13 => (contract::MAX_ASSOCIATIONS, false, Kind::Message(ASSOCIATION)),
});
schema!(IDENTITY {
    1 => (1, false, Kind::Integer32), 2 => (1, false, Kind::Scalar),
});
schema!(PATH {
    1 => (1, false, Kind::Text(MAX_FIELD)),
    2 => (1, false, Kind::Bool), 3 => (1, false, Kind::Bool),
});
schema!(PROJECT {
    1 => (1, false, Kind::Text(MAX_FIELD)), 2 => (1, false, Kind::Text(MAX_FIELD)),
    3 => (1, false, Kind::Text(MAX_FIELD)), 4 => (1, false, Kind::Text(MAX_FIELD)),
    5 => (1, false, Kind::Bool),
});
schema!(DETAILS {
    1 => (1, false, Kind::Message(PATH)), 2 => (1, false, Kind::Message(PATH)),
    3 => (MAX_ARGUMENTS, false, Kind::Text(MAX_FIELD)),
    4 => (1, false, Kind::Message(PROJECT)),
});
schema!(PROCESS {
    1 => (1, false, Kind::Integer32), 2 => (1, false, Kind::Message(IDENTITY)),
    3 => (1, false, Kind::Text(MAX_FIELD)), 4 => (1, false, Kind::Text(MAX_FIELD)),
    5 => (1, false, Kind::Integer32), 6 => (1, false, Kind::Text(MAX_FIELD)),
    7 => (1, false, Kind::Message(DETAILS)), 8 => (1, false, Kind::Message(REPORT)),
});
schema!(MANAGER {
    1 => (1, true, Kind::Message(EMPTY)), 2 => (1, true, Kind::Integer32),
});
schema!(SERVICE {
    1 => (1, false, Kind::Integer32), 2 => (1, false, Kind::Message(MANAGER)),
    3 => (1, false, Kind::Text(255)), 4 => (1, false, Kind::Text(MAX_FIELD)),
    5 => (1, false, Kind::Text(MAX_FIELD)), 6 => (1, false, Kind::Integer32),
    7 => (contract::MAX_ASSOCIATIONS, false, Kind::Text(255)),
});
schema!(COMPOSE {
    1 => (1, false, Kind::Text(MAX_FIELD)), 2 => (1, false, Kind::Text(MAX_FIELD)),
    3 => (1, false, Kind::Text(MAX_FIELD)), 4 => (1, false, Kind::Message(PROJECT)),
});
schema!(PUBLICATION {
    1 => (1, false, Kind::Text(MAX_FIELD)), 2 => (1, false, Kind::Text(MAX_FIELD)),
    3 => (1, false, Kind::Text(MAX_FIELD)), 4 => (1, false, Kind::Text(MAX_FIELD)),
    5 => (1, false, Kind::Integer32), 6 => (1, false, Kind::Message(ENDPOINT)),
    7 => (1, false, Kind::Integer32), 8 => (1, false, Kind::Text(16)),
    9 => (1, false, Kind::Message(COMPOSE)),
});
schema!(ISSUE {
    1 => (1, false, Kind::Integer32), 2 => (1, false, Kind::Scalar),
    3 => (1, false, Kind::Text(contract::MAX_DIAGNOSTIC)),
});
schema!(REPORT {
    1 => (1, false, Kind::Integer32),
    2 => (contract::MAX_ISSUES, false, Kind::Message(ISSUE)),
});
schema!(COVERAGE {
    1 => (1, false, Kind::Message(REPORT)), 2 => (1, false, Kind::Message(REPORT)),
    3 => (1, false, Kind::Message(REPORT)), 4 => (1, false, Kind::Message(REPORT)),
    5 => (1, false, Kind::Message(REPORT)), 6 => (1, false, Kind::Message(REPORT)),
    7 => (1, false, Kind::Message(REPORT)), 8 => (1, false, Kind::Message(REPORT)),
});
schema!(DIAGNOSTICS {
    1 => (1, false, Kind::Integer32), 2 => (1, false, Kind::Integer32),
    3 => (1, false, Kind::Integer32), 4 => (1, false, Kind::Scalar),
});
schema!(SNAPSHOT {
    1 => (1, false, Kind::Scalar), 2 => (1, false, Kind::Scalar),
    3 => (1, false, Kind::Scalar), 4 => (1, false, Kind::Message(SCOPE)),
    5 => (MAX_LISTENERS, false, Kind::Message(SOCKET)),
    6 => (contract::MAX_IDENTITIES, false, Kind::Message(PROCESS)),
    7 => (contract::MAX_SERVICES, false, Kind::Message(SERVICE)),
    8 => (MAX_LISTENERS, false, Kind::Message(PUBLICATION)),
    9 => (1, false, Kind::Message(COVERAGE)),
    10 => (1, false, Kind::Text(MAX_FIELD)),
    11 => (1, false, Kind::Message(DIAGNOSTICS)),
});
