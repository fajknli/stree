// src/protocol/mod.rs

use std::collections::HashMap;
use std::io::BufRead;
use std::fs::File;
use std::io::{BufReader};

/// 四列实体行
#[derive(Debug, Clone)]
pub struct Entity {
    pub id: String,
    pub display: String,
    pub path: String,
    pub tags: String,
}

/// 单条父子关联
#[derive(Debug, Clone)]
pub struct Relation {
    pub parent_id: String,
    pub child_id: String,
}

/// 完整解析结果
#[derive(Debug, Clone)]
pub struct Dataset {
    pub version: u64,
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
    pub entity_map: HashMap<String, Entity>,
    pub child_index: HashMap<String, Vec<String>>,
}

impl Dataset {
    pub fn new() -> Self {
        Self {
            version: 0,
            entities: Vec::new(),
            relations: Vec::new(),
            entity_map: HashMap::new(),
            child_index: HashMap::new(),
        }
    }
}

/// 解析 stdin 全部内容为 Dataset
pub fn parse_entities<R: BufRead>(reader: R) -> anyhow::Result<Dataset> {
    let mut dataset = Dataset::new();

    for (line_num, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let fields: Vec<&str> = line.split('\t').collect();

        let start_idx = if fields.len() >= 5 && fields[0].starts_with("VERSION:") {
            let version_str = fields[0].strip_prefix("VERSION:").unwrap_or("0");
            if let Ok(v) = version_str.parse::<u64>() {
                dataset.version = v;
            }
            1
        } else {
            0
        };

        if fields.len() < start_idx + 4 {
            eprintln!(
                "[WARN] 第 {} 行字段不足4列，已跳过: {}",
                line_num + 1,
                line
            );
            continue;
        }

        // 【防御性增强】强制 trim，清除可能混入的 \r 和首尾空格
        let id = fields[start_idx].trim().to_string();
        let display = fields[start_idx + 1].trim().to_string();
        let path = fields[start_idx + 2].trim().to_string();
        let tags = fields[start_idx + 3].trim().to_string();

        if id.is_empty() {
            eprintln!(
                "[WARN] 第 {} 行 ID 为空，已跳过: {}",
                line_num + 1,
                line
            );
            continue;
        }

        let entity = Entity {
            id: id.clone(),
            display,
            path,
            tags,
        };

        // 【防御性增强】去重时保留最后一个，防止 Vec 膨胀污染后续搜索
        if dataset.entity_map.contains_key(&id) {
            if let Some(pos) = dataset.entities.iter().position(|e| e.id == id) {
                dataset.entities[pos] = entity.clone();
            }
        } else {
            dataset.entities.push(entity.clone());
        }
        dataset.entity_map.insert(id, entity);
    }

    Ok(dataset)
}

/// 解析关联表文件
pub fn parse_relations(relations_path: Option<&str>) -> anyhow::Result<Vec<Relation>> {
    let mut relations = Vec::new();

    let path = match relations_path {
        Some(p) if !p.is_empty() => p,
        _ => return Ok(relations),
    };

    let file = File::open(path)?;
    let reader = BufReader::new(file);

    for (line_num, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let fields: Vec<&str> = line.split('\t').collect();

        if fields.len() < 2 {
            eprintln!(
                "[WARN] 关联表第 {} 行字段不足2列，已跳过: {}",
                line_num + 1,
                line
            );
            continue;
        }

        let parent_id = fields[0].trim().to_string();
        let child_id = fields[1].trim().to_string();

        if parent_id.is_empty() || child_id.is_empty() {
            eprintln!(
                "[WARN] 关联表第 {} 行包含空ID，已跳过: {}",
                line_num + 1,
                line
            );
            continue;
        }

        relations.push(Relation { parent_id, child_id });
    }

    Ok(relations)
}

/// 从 Dataset 构建 child_index
pub fn build_child_index(relations: &[Relation]) -> HashMap<String, Vec<String>> {
    let mut index: HashMap<String, Vec<String>> = HashMap::new();

    for rel in relations {
        let children = index
            .entry(rel.parent_id.clone())
            .or_insert_with(Vec::new);
        if !children.contains(&rel.child_id) {
            children.push(rel.child_id.clone());
        }
    }

    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_parse_basic_tsv() {
        let input = "U-01\tRoot note\t/path/to/note.md\tlive\n";
        let dataset = parse_entities(Cursor::new(input)).unwrap();

        assert_eq!(dataset.entities.len(), 1);
        assert_eq!(dataset.entities[0].id, "U-01");
        assert_eq!(dataset.entities[0].display, "Root note");
        assert_eq!(dataset.entities[0].path, "/path/to/note.md");
        assert_eq!(dataset.entities[0].tags, "live");
    }

    #[test]
    fn test_defensive_trim() {
        // 测试 \r 和首尾空格被清除
        let input = "U-01 \t Root note \t /path.md \t live \r\n";
        let dataset = parse_entities(Cursor::new(input)).unwrap();

        assert_eq!(dataset.entities[0].id, "U-01");
        assert_eq!(dataset.entities[0].display, "Root note");
    }

    #[test]
    fn test_empty_id_rejected() {
        let input = "\tDisplay\tPath\tStatus\n";
        let dataset = parse_entities(Cursor::new(input)).unwrap();

        // 空 ID 的行应该被跳过
        assert_eq!(dataset.entities.len(), 0);
    }

    #[test]
    fn test_deduplication() {
        let input = "U-01\tFirst\t/path1.md\tlive\nU-01\tSecond\t/path2.md\tarchived\n";
        let dataset = parse_entities(Cursor::new(input)).unwrap();

        // 重复 ID 应该保留最后一个
        assert_eq!(dataset.entities.len(), 1);
        assert_eq!(dataset.entities[0].display, "Second");
        assert_eq!(dataset.entities[0].tags, "archived");
    }

    #[test]
    fn test_tag_set_parsing() {
        let input = "U-01\tNote\t/path.md\tlive,note,inbox\n";
        let dataset = parse_entities(Cursor::new(input)).unwrap();

        // 第 4 列应该原样保留（标签集由样式引擎解析）
        assert_eq!(dataset.entities[0].tags, "live,note,inbox");
    }
}
