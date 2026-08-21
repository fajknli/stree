// src/tree/mod.rs

use crate::protocol::{Dataset, Entity};
use std::collections::HashMap;

/// 内存树节点
#[derive(Debug, Clone)]
pub struct TreeNode {
    pub entity: Entity,
    pub children: Vec<TreeNode>,
    pub depth: usize, // 节点深度缓存
}

impl TreeNode {
    pub fn new(entity: Entity, depth: usize) -> Self {
        Self {
            entity,
            children: Vec::new(),
            depth,
        }
    }

    pub fn has_child(&self, child_id: &str) -> bool {
        self.children.iter().any(|c| c.entity.id == child_id)
    }

    pub fn find_node(&self, target_id: &str) -> Option<&TreeNode> {
        if self.entity.id == target_id {
            return Some(self);
        }
        for child in &self.children {
            if let Some(found) = child.find_node(target_id) {
                return Some(found);
            }
        }
        None
    }

    pub fn collect_ids(&self) -> Vec<String> {
        let mut ids = vec![self.entity.id.clone()];
        for child in &self.children {
            ids.extend(child.collect_ids());
        }
        ids
    }

    pub fn depth(&self) -> usize {
        self.children.iter().map(|c| c.depth() + 1).max().unwrap_or(0)
    }

    pub fn size(&self) -> usize {
        1 + self.children.iter().map(|c| c.size()).sum::<usize>()
    }
}

/// 从 Dataset 构建内存树
pub fn build_tree(dataset: &Dataset) -> Vec<TreeNode> {
    let mut roots = Vec::new();

    let mut all_child_ids = std::collections::HashSet::new();
    for child_ids in dataset.child_index.values() {
        for child_id in child_ids {
            all_child_ids.insert(child_id.clone());
        }
    }

    // 直接从 entities 数组按原始输入顺序提取根节点
    let root_ids: Vec<String> = dataset
        .entities
        .iter()
        .map(|e| e.id.clone())
        .filter(|id| !all_child_ids.contains(id))
        .collect();

    for root_id in root_ids {
        if let Some(entity) = dataset.entity_map.get(&root_id) {
            // 初始化祖先足迹，防止图内出现环导致栈溢出
            let mut visited = std::collections::HashSet::new();
            visited.insert(root_id.clone());

            let root = TreeNode {
                entity: entity.clone(),
                children: build_children(&root_id, &dataset.entity_map, &dataset.child_index, 0, &mut visited),
                depth: 0,
            };
            roots.push(root);
        }
    }

    roots
}

/// 递归构建子树，带防栈溢出足迹
fn build_children(
    parent_id: &str,
    entity_map: &HashMap<String, Entity>,
    child_index: &HashMap<String, Vec<String>>,
    parent_depth: usize,
    // 传入祖先足迹，记录当前递归链路中走过的节点
    visited: &mut std::collections::HashSet<String>,
) -> Vec<TreeNode> {
    let mut children = Vec::new();

    if let Some(child_ids) = child_index.get(parent_id) {
        for child_id in child_ids {
            // 【防御核心】如果发现当前子节点已在祖先链路中，说明出现环，直接跳过截断！
            if visited.contains(child_id) {
                eprintln!("[WARN] 检测到循环引用，已截断渲染: {} -> {}", parent_id, child_id);
                continue;
            }

            if let Some(entity) = entity_map.get(child_id) {
                // 克隆足迹给子树，因为兄弟节点之间不共享路径
                let mut child_visited = visited.clone();
                child_visited.insert(child_id.clone());

                let child_tree = TreeNode {
                    entity: entity.clone(),
                    children: build_children(child_id, entity_map, child_index, parent_depth + 1, &mut child_visited),
                    depth: parent_depth + 1,
                };
                children.push(child_tree);
            }
        }
    }
    children
}

pub fn find_node_in_roots<'a>(roots: &'a [TreeNode], target_id: &str) -> Option<&'a TreeNode> {
    for root in roots {
        if let Some(found) = root.find_node(target_id) {
            return Some(found);
        }
    }
    None
}

pub fn collect_all_ids(roots: &[TreeNode]) -> Vec<String> {
    let mut ids = Vec::new();
    for root in roots {
        ids.extend(root.collect_ids());
    }
    ids
}

pub fn total_size(roots: &[TreeNode]) -> usize {
    roots.iter().map(|r| r.size()).sum()
}
