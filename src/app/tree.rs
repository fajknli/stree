// src/app/tree.rs

use crate::protocol::Dataset;
use crate::tree::TreeNode;
use std::collections::HashSet;

#[derive(Debug)]
pub struct TreeState {
    pub dataset: Dataset,
    pub root_tree: Vec<TreeNode>,
    pub selected_id: Option<String>,
    pub expanded_ids: HashSet<String>,
    pub marked_ids: HashSet<String>,
    pub markable: bool,
    pub visible_ids: Vec<String>,
    pub visible_depths: Vec<usize>,
    pub selected_idx: usize,
    pub source_cmd: Option<String>,
    pub relations_path: Option<String>,
    pub click_to_fire: bool,   // click: 前缀
    pub focus_to_fire: bool,   // focus: 前缀
    pub search_query: Option<String>, // 【新增】搜索状态
    pub h_scroll: usize,
    pub v_scroll: usize,
}

impl TreeState {
    pub fn rebuild_visible_ids(&mut self) {
        self.visible_ids.clear();
        self.visible_depths.clear();

        if let Some(query) = &self.search_query {
            if !query.is_empty() {
                let matched = crate::search::match_entities(&self.dataset.entities, query);
                for root in &self.root_tree {
                    Self::collect_matched(root, &matched, &mut self.visible_ids, &mut self.visible_depths);
                }
                return;
            }
        }

        for root in &self.root_tree {
            Self::collect_visible(root, &self.expanded_ids, &mut self.visible_ids, &mut self.visible_depths);
        }
    }

    fn collect_visible(
        node: &TreeNode,
        expanded_ids: &HashSet<String>,
        visible_ids: &mut Vec<String>,
        visible_depths: &mut Vec<usize>,
    ) {
        visible_ids.push(node.entity.id.clone());
        visible_depths.push(node.depth);
        if expanded_ids.contains(&node.entity.id) {
            for child in &node.children {
                Self::collect_visible(child, expanded_ids, visible_ids, visible_depths);
            }
        }
    }

    fn collect_matched(
        node: &TreeNode,
        matched: &std::collections::HashSet<String>,
        visible_ids: &mut Vec<String>,
        visible_depths: &mut Vec<usize>,
    ) {
        if matched.contains(&node.entity.id) {
            visible_ids.push(node.entity.id.clone());
            visible_depths.push(node.depth);
        }
        for child in &node.children {
            Self::collect_matched(child, matched, visible_ids, visible_depths);
        }
    }

    pub fn get_selected_entity(&self) -> Option<&crate::protocol::Entity> {
        let id = self.selected_id.as_ref()?;
        self.dataset.entity_map.get(id)
    }

    pub fn get_marked_entities(&self) -> Vec<&crate::protocol::Entity> {
        self.dataset.entities.iter()
            .filter(|e| self.marked_ids.contains(&e.id) && !e.id.is_empty())
            .collect()
    }

    pub fn move_up(&mut self) {
        if self.visible_ids.is_empty() { return; }
        if self.selected_idx > 0 {
            self.selected_idx -= 1;
            self.selected_id = Some(self.visible_ids[self.selected_idx].clone());
        }
    }

    pub fn move_down(&mut self) {
        if self.visible_ids.is_empty() { return; }
        if self.selected_idx < self.visible_ids.len().saturating_sub(1) {
            self.selected_idx += 1;
            self.selected_id = Some(self.visible_ids[self.selected_idx].clone());
        }
    }

    pub fn toggle_expand(&mut self) {
        let target_id = self.selected_id.clone();
        if let Some(id) = target_id {
            let has_children = crate::tree::find_node_in_roots(&self.root_tree, &id)
                .map(|n| !n.children.is_empty())
                .unwrap_or(false);
            if has_children {
                if self.expanded_ids.contains(&id) {
                    self.expanded_ids.remove(&id);
                } else {
                    self.expanded_ids.insert(id.clone());
                }
                self.rebuild_visible_ids();
                self.select_id(&id);
            }
        }
    }

    pub fn toggle_mark(&mut self) {
        if !self.markable { return; }
        if let Some(id) = self.selected_id.clone() {
            if self.marked_ids.contains(&id) {
                self.marked_ids.remove(&id);
            } else {
                self.marked_ids.insert(id);
            }
            if self.selected_idx < self.visible_ids.len().saturating_sub(1) {
                self.selected_idx += 1;
                self.selected_id = Some(self.visible_ids[self.selected_idx].clone());
            }
        }
    }

    pub fn jump_to_top(&mut self) {
        if self.visible_ids.is_empty() { return; }
        self.selected_idx = 0;
        self.selected_id = Some(self.visible_ids[0].clone());
    }

    pub fn jump_to_bottom(&mut self) {
        if self.visible_ids.is_empty() { return; }
        self.selected_idx = self.visible_ids.len().saturating_sub(1);
        self.selected_id = Some(self.visible_ids.last().unwrap().clone());
    }

    pub fn select_id(&mut self, id: &str) {
        if self.dataset.entity_map.contains_key(id) {
            self.selected_id = Some(id.to_string());
            if let Some(idx) = self.visible_ids.iter().position(|v| v == id) {
                self.selected_idx = idx;
            } else {
                // 【修复 Bug #12】节点存在但不可见（被折叠），保持选中 ID 不变，但不强行跳到第一项
                // 不修改 selected_idx，防止视图跳动
            }
        } else {
            // ID 不在数据集中（例如数据集更新丢失了该节点），重置到第一个可见项
            self.selected_idx = 0;
            self.selected_id = self.visible_ids.first().cloned();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Dataset, Entity};

    fn create_test_tree() -> TreeState {
        let mut dataset = Dataset::new();
        dataset.entities.push(Entity {
            id: "U-01".into(),
            display: "Root".into(),
            path: "/root.md".into(),
            tags: "live".into(),
        });
        dataset.entity_map.insert("U-01".into(), dataset.entities[0].clone());

        let root_tree = vec![TreeNode::new(dataset.entities[0].clone(), 0)];

        let mut state = TreeState {
            dataset,
            markable: true,
            root_tree,
            selected_id: None,
            expanded_ids: HashSet::new(),
            marked_ids: HashSet::new(),
            visible_ids: vec!["U-01".into()],
            visible_depths: vec![0],
            selected_idx: 0,
            source_cmd: None,
            relations_path: None,
            click_to_fire: false,
            focus_to_fire: false,
            search_query: None,
        };
        state.select_id("U-01");
        state
    }

    #[test]
    fn test_select_id_fallback_when_invisible() {
        let mut state = create_test_tree();
        state.visible_ids.clear();
        state.visible_ids.push("U-02".into());
        state.select_id("U-01");
        assert_eq!(state.selected_idx, 0);
        assert_eq!(state.selected_id, Some("U-02".into()));
    }

    #[test]
    fn test_select_id_nonexistent() {
        let mut state = create_test_tree();
        state.select_id("NONEXISTENT");
        assert_eq!(state.selected_idx, 0);
        assert_eq!(state.selected_id, Some("U-01".into()));
    }
}
