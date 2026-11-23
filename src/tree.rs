use std::collections::{BTreeMap, HashSet};

use crate::sara::{ItemSummary, ItemType, SearchHit};

pub enum RowKind {
    Group {
        type_id: String,
        name: String,
        count: usize,
    },
    Document(ItemSummary),
}

pub struct Row {
    parent: Option<usize>,
    pub kind: RowKind,
}

#[derive(Default)]
pub struct Tree {
    rows: Vec<Row>,
    collapsed: HashSet<String>,
    pub count: usize,
}

impl Tree {
    pub fn new(hits: Vec<SearchHit>, item_types: &[ItemType]) -> Self {
        let mut by_type = BTreeMap::<String, Vec<ItemSummary>>::new();
        let count = hits.len();
        for hit in hits {
            by_type.entry(hit.type_id).or_default().push(hit.item);
        }

        let mut tree = Self {
            count,
            ..Self::default()
        };
        for item_type in item_types {
            if let Some(items) = by_type.remove(&item_type.id) {
                tree.push_group(item_type.id.clone(), item_type.display_name.clone(), items);
            }
        }
        for (type_id, items) in by_type {
            tree.push_group(type_id.clone(), type_id, items);
        }
        tree
    }

    fn push_group(&mut self, type_id: String, name: String, items: Vec<ItemSummary>) {
        let parent = self.rows.len();
        self.rows.push(Row {
            parent: None,
            kind: RowKind::Group {
                type_id,
                name,
                count: items.len(),
            },
        });
        self.rows.extend(items.into_iter().map(|item| Row {
            parent: Some(parent),
            kind: RowKind::Document(item),
        }));
    }

    pub fn preserve_folds(&mut self, previous: &Self) {
        self.collapsed.clone_from(&previous.collapsed);
    }

    pub fn visible(&self) -> impl Iterator<Item = (usize, &Row)> {
        let mut expanded = true;
        self.rows
            .iter()
            .enumerate()
            .filter(move |(_, row)| match &row.kind {
                RowKind::Group { type_id, .. } => {
                    expanded = self.expanded(type_id);
                    true
                }
                RowKind::Document(_) => expanded,
            })
    }

    pub fn row_at(&self, position: usize) -> Option<&Row> {
        self.visible().nth(position).map(|(_, row)| row)
    }

    pub fn first_document(&self) -> Option<usize> {
        self.visible()
            .position(|(_, row)| matches!(row.kind, RowKind::Document(_)))
            .or_else(|| (!self.rows.is_empty()).then_some(0))
    }

    pub fn expanded(&self, type_id: &str) -> bool {
        !self.collapsed.contains(type_id)
    }

    pub fn toggle(&mut self, position: usize) {
        if let Some(Row {
            kind: RowKind::Group { type_id, .. },
            ..
        }) = self.row_at(position)
        {
            let type_id = type_id.clone();
            if !self.collapsed.remove(&type_id) {
                self.collapsed.insert(type_id);
            }
        }
    }

    pub fn left(&mut self, position: usize) -> Option<usize> {
        let (_, row) = self.visible().nth(position)?;
        if matches!(&row.kind, RowKind::Group { type_id, .. } if self.expanded(type_id)) {
            self.toggle(position);
            return Some(position);
        }
        self.position(row.parent?)
    }

    pub fn right(&mut self, position: usize) -> Option<usize> {
        let RowKind::Group { type_id, .. } = &self.row_at(position)?.kind else {
            return None;
        };
        if self.expanded(type_id) {
            Some(position + 1)
        } else {
            self.toggle(position);
            Some(position)
        }
    }

    pub fn reveal(&mut self, id: &str) -> Option<usize> {
        let index = self
            .rows
            .iter()
            .position(|row| matches!(&row.kind, RowKind::Document(item) if item.id == id))?;
        if let Some(parent) = self.rows[index].parent {
            if let RowKind::Group { type_id, .. } = &self.rows[parent].kind {
                self.collapsed.remove(type_id);
            }
        }
        self.position(index)
    }

    pub fn group_position(&self, type_id: &str) -> Option<usize> {
        self.visible().position(|(_, row)| {
            matches!(&row.kind, RowKind::Group { type_id: candidate, .. } if candidate == type_id)
        })
    }

    fn position(&self, index: usize) -> Option<usize> {
        self.visible().position(|(candidate, _)| candidate == index)
    }
}
