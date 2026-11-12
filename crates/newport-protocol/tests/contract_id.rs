#[path = "../contract_id.rs"]
mod contract_id;

use contract_id::{Error, fingerprint};
use prost::Message;
use prost_types::{
    DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet, SourceCodeInfo,
    source_code_info::Location,
};

fn schema() -> FileDescriptorSet {
    FileDescriptorSet {
        file: vec![
            FileDescriptorProto {
                name: Some("root.proto".into()),
                dependency: vec!["dependency.proto".into()],
                ..Default::default()
            },
            FileDescriptorProto {
                name: Some("dependency.proto".into()),
                message_type: vec![DescriptorProto {
                    name: Some("Value".into()),
                    field: vec![FieldDescriptorProto {
                        name: Some("number".into()),
                        number: Some(1),
                        r#type: Some(13),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            },
            FileDescriptorProto {
                name: Some("unrelated.proto".into()),
                ..Default::default()
            },
        ],
    }
}

#[test]
fn file_order_root_order_comments_and_source_locations_do_not_change_identity() {
    let mut files = schema();
    let expected = fingerprint(&files, &["root.proto"]).unwrap();
    files.file[1].source_code_info = Some(SourceCodeInfo {
        location: vec![Location {
            path: vec![4, 0],
            span: vec![10, 1, 11, 1],
            leading_comments: Some("A changed description".into()),
            ..Default::default()
        }],
    });
    files.file.reverse();
    assert_eq!(fingerprint(&files, &["root.proto"]).unwrap(), expected);
    assert_eq!(
        fingerprint(&files, &["dependency.proto", "root.proto"]).unwrap(),
        expected
    );
    assert_eq!(
        fingerprint(&files, &["root.proto", "root.proto"]).unwrap(),
        expected
    );
}

#[test]
fn transitive_contract_changes_change_identity_but_unrelated_files_do_not() {
    let mut files = schema();
    let expected = fingerprint(&files, &["root.proto"]).unwrap();
    files.file[2].package = Some("changed".into());
    assert_eq!(fingerprint(&files, &["root.proto"]).unwrap(), expected);
    files.file[1].message_type[0].field[0].number = Some(2);
    assert_ne!(fingerprint(&files, &["root.proto"]).unwrap(), expected);
    files.file[1].message_type[0].field[0].number = Some(1);
    files.file[1].message_type[0].field[0].r#type = Some(4);
    assert_ne!(fingerprint(&files, &["root.proto"]).unwrap(), expected);
}

#[test]
fn new_imports_and_nested_dependencies_are_included() {
    let mut files = schema();
    let expected = fingerprint(&files, &["root.proto"]).unwrap();
    files.file[1].dependency.push("unrelated.proto".into());
    let with_import = fingerprint(&files, &["root.proto"]).unwrap();
    assert_ne!(with_import, expected);
    files.file[2].package = Some("changed".into());
    assert_ne!(fingerprint(&files, &["root.proto"]).unwrap(), with_import);
}

#[test]
fn incomplete_or_ambiguous_descriptor_sets_fail_the_build() {
    let mut files = schema();
    assert_eq!(
        fingerprint(&files, &["missing.proto"]),
        Err(Error::Missing("missing.proto".into()))
    );
    files.file.remove(1);
    assert_eq!(
        fingerprint(&files, &["root.proto"]),
        Err(Error::Missing("dependency.proto".into()))
    );
    let mut files = schema();
    files.file.push(files.file[0].clone());
    assert_eq!(
        fingerprint(&files, &["root.proto"]),
        Err(Error::Duplicate("root.proto".into()))
    );
    files.file[0].name = None;
    assert_eq!(fingerprint(&files, &["root.proto"]), Err(Error::Unnamed));
}

#[test]
fn shipped_identifiers_match_the_generated_monitor_and_desktop_schemas() {
    let files =
        FileDescriptorSet::decode(newport_protocol::encoding::protobuf::DESCRIPTORS).unwrap();
    let monitor = fingerprint(
        &files,
        &["newport/monitor.proto", "newport/discovery.proto"],
    )
    .unwrap();
    let desktop = fingerprint(&files, &["newport/desktop.proto"]).unwrap();
    assert_eq!(monitor, newport_protocol::monitor::CONTRACT_ID);
    assert_eq!(desktop, newport_protocol::desktop::CONTRACT_ID);
    assert_eq!(monitor.len(), 64);
    assert!(monitor.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_ne!(monitor, desktop);
    assert_eq!(newport_protocol::monitor::VERSION, 1);
    assert_eq!(newport_protocol::desktop::VERSION, 1);
}
