use crate::search::temporal::TimePeriod;
use cas_types::{Entity, Entry, Relationship};
use chrono::{DateTime, Duration, Utc};

pub fn filter_entities_by_time(
    entities: Vec<Entity>,
    period: &TimePeriod,
    include_updated: bool,
) -> Vec<Entity> {
    entities
        .into_iter()
        .filter(|e| period.contains(&e.created) || (include_updated && period.contains(&e.updated)))
        .collect()
}

/// Filter relationships by temporal validity
pub fn filter_relationships_by_time(
    relationships: Vec<Relationship>,
    period: &TimePeriod,
) -> Vec<Relationship> {
    relationships
        .into_iter()
        .filter(|r| period.overlaps_relationship(r))
        .collect()
}

// =============================================================================
// Entry-based Temporal Retrieval (Hindsight-inspired)
// =============================================================================

/// Temporal retrieval result for entries
#[derive(Debug, Clone)]
pub struct TemporalEntryResult {
    /// The entry ID
    pub id: String,
    /// Temporal relevance score (0.0-1.0)
    /// Higher for entries created closer to query time or with matching validity
    pub temporal_score: f32,
    /// How the entry relates to the query time
    pub temporal_relation: TemporalRelation,
}

/// How an entry relates to the query time period
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemporalRelation {
    /// Entry was created within the query period
    CreatedDuring,
    /// Entry's validity period overlaps with query period
    ValidDuring,
    /// Entry was created before but still valid
    PrecedesButValid,
    /// Entry was recently accessed during the period
    AccessedDuring,
}

/// Temporal retriever for entries (Hindsight-inspired 4th search channel)
///
/// This retriever scores entries based on their temporal relationship to a query,
/// enabling time-aware search as a parallel channel alongside BM25, semantic, and graph.
pub fn filter_entries_by_time(entries: Vec<Entry>, period: &TimePeriod) -> Vec<Entry> {
    entries
        .into_iter()
        .filter(|e| {
            // Entry is relevant if:
            // 1. Created within period, OR
            // 2. Valid during period (using valid_from/valid_until), OR
            // 3. No validity bounds and created before period end
            if e.known_creation()
                .is_some_and(|created| period.contains(&created))
            {
                return true;
            }

            if let Some(valid_from) = e.valid_from {
                let valid_until = e
                    .valid_until
                    .unwrap_or_else(|| Utc::now() + Duration::days(36500));
                let entry_valid = valid_from <= period.end && valid_until >= period.start;
                if entry_valid {
                    return true;
                }
            } else {
                // No explicit validity - check if created before and no end date
                if e.known_creation()
                    .is_some_and(|created| created <= period.end)
                    && e.valid_until.is_none()
                {
                    return true;
                }
            }

            false
        })
        .collect()
}

/// Get the state of an entity at a specific point in time
#[derive(Debug, Clone)]
pub struct EntitySnapshot {
    /// The entity
    pub entity: Entity,

    /// Relationships that were valid at the snapshot time
    pub relationships: Vec<Relationship>,

    /// The snapshot timestamp
    pub timestamp: DateTime<Utc>,
}

impl EntitySnapshot {
    /// Create a snapshot from an entity and all its relationships,
    /// filtered to those valid at the given timestamp
    pub fn at_time(
        entity: Entity,
        all_relationships: Vec<Relationship>,
        timestamp: DateTime<Utc>,
    ) -> Self {
        let period = TimePeriod::new(timestamp, timestamp);
        let relationships = filter_relationships_by_time(all_relationships, &period);

        Self {
            entity,
            relationships,
            timestamp,
        }
    }
}

/// History of an entity over time
#[derive(Debug, Clone)]
pub struct EntityHistory {
    /// The entity
    pub entity: Entity,

    /// All relationships, ordered by creation time
    pub relationship_history: Vec<RelationshipEvent>,
}

/// A relationship event in history
#[derive(Debug, Clone)]
pub struct RelationshipEvent {
    /// The relationship
    pub relationship: Relationship,

    /// Event type
    pub event: HistoryEventType,

    /// When this event occurred
    pub timestamp: DateTime<Utc>,
}

/// Types of history events
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryEventType {
    /// Relationship was created/established
    Created,
    /// Relationship became valid
    BecameValid,
    /// Relationship ended/became invalid
    Ended,
}

impl EntityHistory {
    /// Build history from an entity and its relationships
    pub fn from_relationships(entity: Entity, relationships: Vec<Relationship>) -> Self {
        let mut events = Vec::new();

        for rel in relationships {
            // Created event
            events.push(RelationshipEvent {
                relationship: rel.clone(),
                event: HistoryEventType::Created,
                timestamp: rel.created,
            });

            // Valid from event (if different from created)
            if let Some(valid_from) = rel.valid_from {
                if valid_from != rel.created {
                    events.push(RelationshipEvent {
                        relationship: rel.clone(),
                        event: HistoryEventType::BecameValid,
                        timestamp: valid_from,
                    });
                }
            }

            // Ended event
            if let Some(valid_until) = rel.valid_until {
                events.push(RelationshipEvent {
                    relationship: rel.clone(),
                    event: HistoryEventType::Ended,
                    timestamp: valid_until,
                });
            }
        }

        // Sort by timestamp
        events.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

        Self {
            entity,
            relationship_history: events,
        }
    }
}
