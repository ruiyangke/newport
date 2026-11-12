use super::*;

fn enumeration<T: std::fmt::Debug + PartialEq>(
    values: &[T],
    encode: fn(&T) -> i32,
    decode: fn(i32) -> Result<T, Error>,
) {
    for value in values {
        assert!(encode(value) > 0);
        assert_eq!(&decode(encode(value)).unwrap(), value);
    }
    for unknown in [0, -1, i32::MAX] {
        assert!(decode(unknown).is_err());
    }
}

#[test]
fn enums_round_trip_known_values_and_reject_unspecified_or_unknown_values() {
    use contract::*;
    enumeration(
        &[
            Side::HeadToIndex,
            Side::IndexToWorktree,
            Side::HeadToWorktree,
        ],
        side_to_wire,
        side_from_wire,
    );
    enumeration(
        &[ResetMode::Soft, ResetMode::Mixed, ResetMode::Hard],
        reset_mode_to_wire,
        reset_mode_from_wire,
    );
    enumeration(
        &[DiscardSource::Index, DiscardSource::Head],
        discard_source_to_wire,
        discard_source_from_wire,
    );
    enumeration(
        &[ConflictSide::Base, ConflictSide::Ours, ConflictSide::Theirs],
        conflict_side_to_wire,
        conflict_side_from_wire,
    );
    enumeration(
        &[DiffLineEncoding::TupleV1],
        diff_line_encoding_to_wire,
        diff_line_encoding_from_wire,
    );
    enumeration(
        &[
            OperationState::Running,
            OperationState::Succeeded,
            OperationState::Failed,
            OperationState::NeedsResolution,
            OperationState::OutcomeUnknown,
            OperationState::ReviewedUnknown,
        ],
        operation_state_to_wire,
        operation_state_from_wire,
    );
}
