// src/search/mod.rs

use crate::protocol::Entity;
use std::collections::HashSet;

/// 内容层模糊匹配引擎
///
/// 遍历实体列表，对三列内容字段（id, display, path）进行不区分大小写的子串匹配。
///
/// 【核心契约】：第 4 列（status/tags）属于元数据层，**绝对不参与搜索**。
/// 这是为了避免“幽灵匹配”（例如：搜索 "li" 不会因为隐藏的 "live" 标签而导致所有节点全亮）。
/// 业务层若希望某些标签被搜索到，应将其拼接到 display 字段中。
///
/// # 参数
/// - `entities`: 原始实体数据切片（连续内存，高缓存命中率）
/// - `query`: 搜索词。如果为空，则返回空集合（由 TreeState 决定是否开启过滤）
///
/// # 返回
/// 匹配的实体 ID 集合 (HashSet<String>)
pub fn match_entities(entities: &[Entity], query: &str) -> HashSet<String> {
    let mut matched = HashSet::new();

    // 空查询直接返回空集合，TreeState 会将其视为“无过滤”状态
    if query.trim().is_empty() {
        return matched;
    }

    let lower_query = query.to_lowercase();

    for entity in entities {
        // 仅搜索内容层（id, display, path），不搜索元数据层
        // 【修复】去掉路径末尾的 .md 扩展名，防止搜索 "md" 或 "." 匹配到所有文件
        let searchable_path = entity.path.trim_end_matches(".md");

        let is_match = entity.id.to_lowercase().contains(&lower_query)
            || entity.display.to_lowercase().contains(&lower_query)
            || searchable_path.to_lowercase().contains(&lower_query);

        if is_match {
            matched.insert(entity.id.clone());
        }
    }

    matched
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Entity;

    fn create_test_entities() -> Vec<Entity> {
        vec![
            Entity { id: "U-01".into(), display: "Root Note".into(), path: "/tmp/root.md".into(), tags: "live".into() },
            Entity { id: "U-02".into(), display: "Child Note A".into(), path: "/tmp/child_a.md".into(), tags: "live".into() },
            Entity { id: "U-03".into(), display: "Archived Item".into(), path: "/tmp/old.md".into(), tags: "archived".into() },
            Entity { id: "SP-01".into(), display: "Note with spaces".into(), path: "/tmp/my note.md".into(), tags: "live,idea".into() },
        ]
    }

    // ... 保留 test_empty_query_returns_empty, test_match_by_display, test_match_by_path_with_spaces, test_case_insensitive, test_no_match ...

    // 【修改】替换掉原来错误的 test_match_by_status 和 test_multiple_matches
    #[test]
    fn test_multiple_matches() {
        let entities = create_test_entities();
        // "Note" 匹配 U-01, U-02, SP-01 的 display 字段
        let result = match_entities(&entities, "Note");
        assert_eq!(result.len(), 3);
        assert!(result.contains("U-01"));
        assert!(result.contains("U-02"));
        assert!(result.contains("SP-01"));
    }

    /// 【核心契约测试】验证第 4 列（status/tags）不参与搜索
    #[test]
    fn test_status_field_is_ignored() {
        let entities = create_test_entities();

        // 1. 搜索 "idea"，SP-01 的 status 包含 "idea"，但 display 和 path 都不包含。
        // 预期结果：搜不到！因为 status 不参与搜索。
        let result_idea = match_entities(&entities, "idea");
        assert!(result_idea.is_empty(), "status/tags 字段不应参与搜索");

        // 2. 搜索 "live"，前三个节点的 status 都有 "live"，但 display/path 中没有。
        // 预期结果：搜不到！
        let result_live = match_entities(&entities, "live");
        assert!(result_live.is_empty(), "status/tags 字段不应参与搜索");

        // 3. 搜索 "archived"，U-03 能搜到，是因为它的 display 是 "Archived Item"，
        // 而不是因为它的 status 是 "archived"。
        let result_archived = match_entities(&entities, "archived");
        assert_eq!(result_archived.len(), 1);
        assert!(result_archived.contains("U-03"));
    }
}
