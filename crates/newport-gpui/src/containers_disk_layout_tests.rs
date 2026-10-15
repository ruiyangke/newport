use crate::containers::tests::{draw, fixture, select};
use gpui::{Role, TestAppContext, px, size};
use gpui_kit as gpui;

#[gpui::test]
fn disk_inventory_exposes_cells_summary_and_compact_metadata(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "disk");
    for width in [1200., 440.] {
        cx.simulate_resize(size(px(width), px(1500.)));
        draw(cx);
        cx.update(|window, _| {
            let nodes = gpui_kit::base::test_support::snapshots(window);
            assert!(nodes.iter().any(
                |n| n.role() == Some(Role::Table) && n.label() == Some("Docker disk inventory")
            ));
            assert!(nodes.iter().any(|n| n.role() == Some(Role::Status)
                && n.label().is_some_and(|s| s.starts_with("Docker uses "))));
            assert!(nodes.iter().any(|n| n.role() == Some(Role::Region)
                && n.label().is_some_and(|s| {
                    s.starts_with("Images: 2.9 MiB, 2 images, 976.6 KiB can be freed")
                })));
            for label in ["Size", "Image", "Volume"] {
                assert!(
                    nodes
                        .iter()
                        .any(|n| n.role() == Some(Role::ColumnHeader) && n.label() == Some(label)),
                    "missing {label} at {width}"
                );
            }
            assert!(
                nodes
                    .iter()
                    .any(|n| n.role() == Some(Role::Cell) && n.label() == Some("1.9 MiB"))
            );
            assert!(nodes.iter().any(|n| {
                n.role() == Some(Role::Cell)
                    && n.label()
                        .is_some_and(|s| s.contains("stack-data") && s.contains("project stack"))
            }));
            let image = nodes
                .iter()
                .find(|n| {
                    n.role() == Some(Role::Cell)
                        && n.label().is_some_and(|s| s.starts_with("fixture/web:1"))
                })
                .unwrap();
            if width == 440. {
                assert!(image.label().unwrap().contains("created"));
                assert!(image.label().unwrap().contains("1 container"));
            } else {
                assert_eq!(image.label(), Some("fixture/web:1"));
                assert!(
                    nodes
                        .iter()
                        .any(|n| n.role() == Some(Role::ColumnHeader)
                            && n.label() == Some("Created"))
                );
            }
        });
    }
    assert_eq!(backend.writes(), 0);
}

#[gpui::test]
fn disk_cards_keep_equal_columns_in_incomplete_rows_and_at_compact_widths(cx: &mut TestAppContext) {
    let (panel, cx, backend) = fixture(cx);
    select(&panel, cx, "disk");
    for width in [980., 900., 800., 620., 560., 1200.] {
        cx.simulate_resize(size(px(width), px(1000.)));
        draw(cx);
        let pane = cx.debug_bounds("containers-disk").unwrap();
        let cards: Vec<_> = [
            "disk-tile-Images",
            "disk-tile-Containers",
            "disk-tile-Volumes",
            "disk-tile-Build cache",
        ]
        .map(|selector| cx.debug_bounds(selector).unwrap())
        .to_vec();
        for card in &cards {
            assert!(
                (card.size.width - cards[0].size.width).abs() <= px(1.),
                "last-row card must not stretch at width{width}: {cards:?}"
            );
            assert!(
                card.left() >= pane.left() && card.right() <= pane.right() + px(1.),
                "cards should fit the pane at width{width}"
            );
        }
        if width == 900. || width == 800. {
            assert_eq!(
                cards[0].top(),
                cards[2].top(),
                "three cards should share first row at width{width}"
            );
            assert!(cards[3].top() > cards[0].top());
            assert_eq!(
                cards[3].left(),
                cards[0].left(),
                "final row aligns to first column"
            );
            assert!((cards[1].left() - cards[0].right() - px(10.)).abs() <= px(1.));
        }
    }
    assert_eq!(backend.writes(), 0);
}
