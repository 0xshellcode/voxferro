//! The outline's menus: sort the objects by name, close or expand branches,
//! hide or show objects and their contents, and the layer menu's sort,
//! visibility and select-by-layer.
//!
//! What each one touches:
//!
//! - "selected" is the object selection, or the selected group or model;
//! - the open group is the one entered with Tab; at the top level it is the
//!   scene itself;
//! - closing and expanding are a view setting of the session: nothing is
//!   saved and nothing goes into the undo history;
//! - hiding writes the node's `_hidden`, saved with the file, as one undo
//!   step like every other edit of the scene graph.

use crate::App;
use voxel_core::graph::TreeKind;

impl App {
    /// The transform nodes the menus act on: the object selection, or the
    /// selected group, or the selected model's own node.
    pub(crate) fn picked_nodes(&self) -> Vec<i32> {
        if self.objects.is_empty() {
            self.target_node().into_iter().collect()
        } else {
            self.objects.clone()
        }
    }

    /// `tree sort | expand [all] | close [inactive] | hide [deep|others] |
    /// show [deep|all] | toggle`
    ///
    /// `hide` and `show` act on the selected objects alone; with `deep`, on
    /// them and everything inside (Hide and Show Sub Objects). `hide others`
    /// hides everything at each level from the scene down to the open group,
    /// and down to each selected object, but the way down and the selection;
    /// `show all` shows every object.
    pub fn tree_command(&mut self, line: &str) -> String {
        let words: Vec<&str> = line.split_whitespace().skip(1).collect();
        let usage = "tree sort | expand [all] | close [inactive] | hide [deep|others] | show [deep|all] | toggle";
        let Some(loaded) = self.scene.as_ref() else { return "nothing open".to_owned() };
        let scene = &loaded.doc.scene;
        let picked = self.picked_nodes();
        let open = self.open_groups.last().copied();
        let groups: Vec<i32> = scene.tree().iter().filter(|r| r.kind == TreeKind::Group).map(|r| r.node).collect();
        let under = |nodes: &[i32]| -> Vec<i32> { nodes.iter().flat_map(|&n| scene.nodes_under(n)).collect() };

        let (nodes, message) = match words.as_slice() {
            ["sort"] => {
                let result = scene.with_children_sorted(open, &picked);
                return match self.exec_graph(result, false) {
                    Ok(()) => {
                        // Sorting renumbers the nodes: picks by id are stale.
                        self.forget_picks();
                        self.collapsed.clear();
                        "sorted by name".to_owned()
                    }
                    Err(e) => format!("error: {e}"),
                };
            }
            ["close"] => {
                self.collapsed.extend(under(&picked).into_iter().filter(|n| groups.contains(n)));
                return "closed".to_owned();
            }
            ["close", "inactive"] => {
                self.collapsed = groups.iter().copied().filter(|g| !self.open_groups.contains(g)).collect();
                return "closed all but the way to the open group".to_owned();
            }
            ["expand"] => {
                for n in under(&picked).iter().chain(&self.open_groups) {
                    self.collapsed.remove(n);
                }
                return "expanded".to_owned();
            }
            ["expand", "all"] => {
                self.collapsed.clear();
                return "expanded all".to_owned();
            }
            [verb @ ("hide" | "show")] => (scene.with_nodes_attribute(&picked, "_hidden", (*verb == "hide").then_some("1")), format!("{verb}n")),
            ["toggle"] => {
                // The first's state decides, as Ctrl+H does there.
                let hidden = picked.first().is_some_and(|&n| scene.tree().iter().any(|r| r.node == n && r.hidden));
                (scene.with_nodes_attribute(&picked, "_hidden", (!hidden).then_some("1")), if hidden { "shown" } else { "hidden" }.to_owned())
            }
            [verb @ ("hide" | "show"), "deep"] => {
                let nodes = under(&picked);
                (scene.with_nodes_attribute(&nodes, "_hidden", (*verb == "hide").then_some("1")), format!("{verb}n with their contents"))
            }
            ["hide", "others"] => {
                if picked.is_empty() {
                    return "nothing selected".to_owned();
                }
                // The way down to the open group and to each selected object
                // -- the outline can pick one deeper than the open group --
                // stays shown...
                let mut keep: Vec<i32> = self.open_groups.clone();
                for &n in &picked {
                    keep.extend(scene.path_to(n));
                }
                // ...and everything else at each level along it is hidden.
                let mut levels = scene.children_of(None);
                for &g in &keep {
                    levels.extend(scene.children_of(Some(g)));
                }
                levels.retain(|n| !keep.contains(n));
                (scene.with_visibility(&levels, &keep), "others hidden".to_owned())
            }
            ["show", "all"] => {
                let all: Vec<i32> = scene.tree().iter().map(|r| r.node).collect();
                (scene.with_nodes_attribute(&all, "_hidden", None), "everything shown".to_owned())
            }
            _ => return usage.to_owned(),
        };
        if picked.is_empty() && !matches!(words.as_slice(), ["show", "all"]) {
            return "nothing selected".to_owned();
        }
        match self.exec_graph(nodes, false) {
            Ok(()) => message,
            Err(e) => format!("error: {e}"),
        }
    }
}
