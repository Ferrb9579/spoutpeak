use accesskit::{NodeId, TreeUpdate};
use accesskit_consumer::{Node, Tree, TreeChangeHandler};
use incular_accessibility::{
    AccessKitProjection, Role, SemanticNode, SemanticNodeId, SemanticState, SemanticsTree,
};
use incular_core::{ArenaId, Rect};

struct IgnoreChanges;

impl TreeChangeHandler for IgnoreChanges {
    fn node_added(&mut self, _: &Node) {}
    fn node_updated(&mut self, _: &Node, _: &Node) {}
    fn focus_moved(&mut self, _: Option<&Node>, _: Option<&Node>) {}
    fn node_removed(&mut self, _: &Node) {}
}

fn insert(tree: &mut SemanticsTree, label: &str, children: Vec<SemanticNodeId>) -> SemanticNodeId {
    tree.insert(SemanticNode {
        id: SemanticNodeId(ArenaId::from_parts(0, 0)),
        role: Role::Button,
        label: Some(label.into()),
        value: None,
        description: None,
        bounds: Rect::default(),
        state: SemanticState::default(),
        actions: vec![],
        children,
    })
}

fn sync(projection: &mut AccessKitProjection, tree: &SemanticsTree) -> TreeUpdate {
    projection.sync(tree, 1.0).unwrap().into_accesskit()
}

fn labels(tree: &Tree) -> Vec<String> {
    tree.state()
        .root()
        .children()
        .map(|node| node.label().unwrap())
        .collect()
}

#[test]
fn all_top_level_nodes_and_descendants_are_connected_on_activation() {
    let mut semantics = SemanticsTree::new();
    let first = insert(&mut semantics, "first", vec![]);
    let child = insert(&mut semantics, "child", vec![]);
    let second = insert(&mut semantics, "second", vec![child]);
    semantics.set_root(Some(second));
    let mut projection = AccessKitProjection::new();
    let tree = Tree::new(sync(&mut projection, &semantics), true);
    assert_eq!(labels(&tree), ["second", "first"]);
    assert_eq!(
        tree.state()
            .root()
            .children()
            .next()
            .unwrap()
            .children()
            .len(),
        1
    );
    assert!(projection.native_node_id(first).is_some());
    assert!(projection.sync(&semantics, 1.0).is_none());
}

#[test]
fn single_root_host_reproduces_the_reported_accesskit_panic() {
    let mut semantics = SemanticsTree::new();
    let first = insert(&mut semantics, "first", vec![]);
    insert(&mut semantics, "second", vec![]);
    semantics.set_root(Some(first));
    let mut projection = AccessKitProjection::new();
    let mut update = sync(&mut projection, &semantics);
    // Restore upstream's single-root host while keeping its published forest.
    let first_native = NodeId(projection.native_node_id(first).unwrap());
    update
        .nodes
        .iter_mut()
        .find(|(id, _)| *id == NodeId(0))
        .unwrap()
        .1
        .set_children(vec![first_native]);
    let panic = std::panic::catch_unwind(|| Tree::new(update, true)).unwrap_err();
    let message = panic.downcast_ref::<String>().unwrap();
    assert!(message.contains("neither in the current tree nor a child"));
}

#[test]
fn incremental_root_insertion_removal_and_reparenting_remain_valid() {
    let mut semantics = SemanticsTree::new();
    let first = insert(&mut semantics, "first", vec![]);
    semantics.set_root(Some(first));
    let mut projection = AccessKitProjection::new();
    let mut tree = Tree::new(sync(&mut projection, &semantics), true);
    let second = insert(&mut semantics, "second", vec![]);
    let third = insert(&mut semantics, "third", vec![]);
    tree.update_and_process_changes(sync(&mut projection, &semantics), &mut IgnoreChanges);
    assert_eq!(labels(&tree), ["first", "second", "third"]);

    // A former host child becomes a descendant without being republished.
    semantics.update_geometry(first, Rect::default(), &[second]);
    tree.update_and_process_changes(sync(&mut projection, &semantics), &mut IgnoreChanges);
    assert_eq!(labels(&tree), ["first", "third"]);
    assert_eq!(
        tree.state()
            .root()
            .children()
            .next()
            .unwrap()
            .children()
            .len(),
        1
    );
    semantics.update_geometry(first, Rect::default(), &[]);
    tree.update_and_process_changes(sync(&mut projection, &semantics), &mut IgnoreChanges);
    assert_eq!(labels(&tree), ["first", "second", "third"]);

    semantics.remove(second);
    tree.update_and_process_changes(sync(&mut projection, &semantics), &mut IgnoreChanges);
    assert_eq!(labels(&tree), ["first", "third"]);
    let replacement = insert(&mut semantics, "replacement", vec![]);
    tree.update_and_process_changes(sync(&mut projection, &semantics), &mut IgnoreChanges);
    assert_eq!(labels(&tree), ["first", "replacement", "third"]);
    assert_ne!(second, replacement);
    assert!(projection.native_node_id(second).is_none());

    semantics.remove(first);
    semantics.remove(third);
    semantics.remove(replacement);
    tree.update_and_process_changes(sync(&mut projection, &semantics), &mut IgnoreChanges);
    assert!(labels(&tree).is_empty());
}

#[test]
fn forest_without_designated_root_supports_focus_and_reactivation() {
    let mut semantics = SemanticsTree::new();
    insert(&mut semantics, "first", vec![]);
    let second = insert(&mut semantics, "second", vec![]);
    let mut focused = semantics.node(second).unwrap().clone();
    focused.state.focused = true;
    semantics.update(second, focused);
    let mut projection = AccessKitProjection::new();
    let mut tree = Tree::new(sync(&mut projection, &semantics), true);
    assert_eq!(labels(&tree), ["first", "second"]);
    assert_eq!(
        tree.state().focus().unwrap().label().as_deref(),
        Some("second")
    );

    projection.deactivate();
    projection.activate();
    tree.update_and_process_changes(sync(&mut projection, &semantics), &mut IgnoreChanges);
    assert_eq!(labels(&tree), ["first", "second"]);
    insert(&mut semantics, "third", vec![]);
    let update = projection.sync(&semantics, 2.0).unwrap().into_accesskit();
    tree.update_and_process_changes(update, &mut IgnoreChanges);
    assert_eq!(labels(&tree), ["first", "second", "third"]);
}
