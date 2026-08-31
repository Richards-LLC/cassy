use chrono::{DateTime, Utc};

use crate::entry::{
    BeliefType, Entry, EntryType, MemoryTier, ObservationType, default_confidence,
    default_importance, default_stability,
};
use crate::scope::Scope;

impl Entry {
    /// Create a new entry with the given content (defaults to project scope)
    pub fn new(id: String, content: String) -> Self {
        Self::with_scope(id, content, Scope::Project)
    }

    /// Create a new entry with explicit scope
    pub fn with_scope(id: String, content: String, scope: Scope) -> Self {
        Self {
            id,
            scope,
            entry_type: EntryType::default(),
            observation_type: None,
            source_ids: Vec::new(),
            tags: Vec::new(),
            created: Utc::now(),
            content,
            raw_content: None,
            compressed: false,
            memory_tier: MemoryTier::Working,
            title: None,
            helpful_count: 0,
            harmful_count: 0,
            last_accessed: None,
            archived: false,
            session_id: None,
            source_tool: None,
            pending_extraction: false,
            pending_embedding: true,
            stability: default_stability(),
            access_count: 0,
            importance: default_importance(),
            valid_from: None,
            valid_until: None,
            review_after: None,
            last_reviewed: None,
            domain: None,
            belief_type: BeliefType::Fact,
            confidence: default_confidence(),
            branch: None,
            team_id: None,
            share: None,
        }
    }

    /// Create a new opinion entry (Hindsight-inspired)
    pub fn new_opinion(id: String, content: String, confidence: f32) -> Self {
        Self {
            id,
            scope: Scope::Project,
            entry_type: EntryType::Learning,
            observation_type: None,
            source_ids: Vec::new(),
            tags: vec!["opinion".to_string()],
            created: Utc::now(),
            content,
            raw_content: None,
            compressed: false,
            memory_tier: MemoryTier::Working,
            title: None,
            helpful_count: 0,
            harmful_count: 0,
            last_accessed: None,
            archived: false,
            session_id: None,
            source_tool: None,
            pending_extraction: false,
            pending_embedding: true,
            stability: 0.4, // Slightly lower initial stability for opinions
            access_count: 0,
            importance: 0.6, // Opinions are moderately important
            valid_from: None,
            valid_until: None,
            review_after: None,
            last_reviewed: None,
            domain: None,
            belief_type: BeliefType::Opinion,
            confidence: confidence.clamp(0.0, 1.0),
            branch: None,
            team_id: None,
            share: None,
        }
    }

    /// Create a new hypothesis entry (Hindsight-inspired)
    pub fn new_hypothesis(id: String, content: String, confidence: f32) -> Self {
        Self {
            id,
            scope: Scope::Project,
            entry_type: EntryType::Learning,
            observation_type: None,
            source_ids: Vec::new(),
            tags: vec!["hypothesis".to_string()],
            created: Utc::now(),
            content,
            raw_content: None,
            compressed: false,
            memory_tier: MemoryTier::Working,
            title: None,
            helpful_count: 0,
            harmful_count: 0,
            last_accessed: None,
            archived: false,
            session_id: None,
            source_tool: None,
            pending_extraction: false,
            pending_embedding: true,
            stability: 0.3, // Lower stability - hypotheses need verification
            access_count: 0,
            importance: 0.5,
            valid_from: None,
            valid_until: None,
            review_after: None,
            last_reviewed: None,
            domain: None,
            belief_type: BeliefType::Hypothesis,
            confidence: confidence.clamp(0.0, 1.0),
            branch: None,
            team_id: None,
            share: None,
        }
    }

    /// Create a new observation entry from a hook
    pub fn new_observation(
        id: String,
        content: String,
        session_id: String,
        source_tool: String,
    ) -> Self {
        // Infer observation type from source tool
        let obs_type = infer_observation_type(&source_tool, &content);

        Self {
            id,
            scope: Scope::Project, // Observations are always project-scoped
            entry_type: EntryType::Observation,
            observation_type: Some(obs_type),
            source_ids: Vec::new(),
            tags: Vec::new(),
            created: Utc::now(),
            content,
            raw_content: None,
            compressed: false,
            memory_tier: MemoryTier::Working,
            title: None,
            helpful_count: 0,
            harmful_count: 0,
            last_accessed: None,
            archived: false,
            session_id: Some(session_id),
            source_tool: Some(source_tool),
            pending_extraction: true,
            pending_embedding: true,
            stability: 0.3, // Lower initial stability for observations
            access_count: 0,
            importance: 0.3, // Lower initial importance for auto-captured observations
            valid_from: None,
            valid_until: None,
            review_after: None,
            last_reviewed: None,
            domain: None,
            belief_type: BeliefType::Fact, // Observations are typically factual
            confidence: 0.8,               // Slightly lower confidence for auto-captured
            branch: None,
            team_id: None,
            share: None,
        }
    }

    /// Create an observation with a specific type
    pub fn new_typed_observation(
        id: String,
        content: String,
        session_id: String,
        source_tool: String,
        observation_type: ObservationType,
    ) -> Self {
        Self {
            id,
            scope: Scope::Project, // Observations are always project-scoped
            entry_type: EntryType::Observation,
            observation_type: Some(observation_type),
            source_ids: Vec::new(),
            tags: Vec::new(),
            created: Utc::now(),
            content,
            raw_content: None,
            compressed: false,
            memory_tier: MemoryTier::Working,
            title: None,
            helpful_count: 0,
            harmful_count: 0,
            last_accessed: None,
            archived: false,
            session_id: Some(session_id),
            source_tool: Some(source_tool),
            pending_extraction: true,
            pending_embedding: true,
            stability: 0.3,
            access_count: 0,
            importance: 0.3,
            valid_from: None,
            valid_until: None,
            review_after: None,
            last_reviewed: None,
            domain: None,
            belief_type: BeliefType::Fact,
            confidence: 0.8,
            branch: None,
            team_id: None,
            share: None,
        }
    }

    /// Check if this entry is currently valid (within its temporal bounds)
    ///
    /// Returns true if:
    /// - No temporal bounds are set (always valid)
    /// - Current time is after valid_from (if set)
    /// - Current time is before valid_until (if set)
    pub fn is_temporally_valid(&self) -> bool {
        let now = Utc::now();

        // Check valid_from
        if let Some(from) = self.valid_from {
            if now < from {
                return false;
            }
        }

        // Check valid_until
        if let Some(until) = self.valid_until {
            if now > until {
                return false;
            }
        }

        true
    }

    /// Check if this entry is expired (past its valid_until date)
    pub fn is_expired(&self) -> bool {
        if let Some(until) = self.valid_until {
            Utc::now() > until
        } else {
            false
        }
    }

    /// Set temporal validity bounds
    pub fn set_validity(
        &mut self,
        valid_from: Option<DateTime<Utc>>,
        valid_until: Option<DateTime<Utc>>,
    ) {
        self.valid_from = valid_from;
        self.valid_until = valid_until;
    }

    /// Demote entry to a lower memory tier
    pub fn demote_tier(&mut self) {
        // InContext tier is special - use unpin() to remove from in-context
        self.memory_tier = match self.memory_tier {
            MemoryTier::InContext => MemoryTier::InContext, // Don't auto-demote pinned
            MemoryTier::Working => MemoryTier::Cold,
            MemoryTier::Cold => MemoryTier::Archive,
            MemoryTier::Archive => MemoryTier::Archive,
        };
    }

    /// Promote an accessed entry to the active working tier.
    /// Note: Use pin() to promote to InContext tier.
    pub fn promote_tier(&mut self) {
        self.memory_tier = match self.memory_tier {
            MemoryTier::Archive => MemoryTier::Working,
            MemoryTier::Cold => MemoryTier::Working,
            MemoryTier::Working => MemoryTier::Working,
            MemoryTier::InContext => MemoryTier::InContext,
        };
    }

    /// Pin entry to in-context tier (always injected into sessions)
    pub fn pin(&mut self) {
        self.memory_tier = MemoryTier::InContext;
    }

    /// Unpin entry from in-context tier (move to working memory)
    pub fn unpin(&mut self) {
        if self.memory_tier == MemoryTier::InContext {
            self.memory_tier = MemoryTier::Working;
        }
    }

    /// Check if entry is pinned (in-context tier)
    pub fn is_pinned(&self) -> bool {
        self.memory_tier == MemoryTier::InContext
    }

    /// Calculate the feedback score (helpful - harmful)
    pub fn feedback_score(&self) -> i32 {
        self.helpful_count - self.harmful_count
    }

    /// Get a short preview of the content
    pub fn preview(&self, max_len: usize) -> String {
        crate::preview::truncate_preview(&self.content, max_len)
    }

    /// Get display title (title or content preview)
    pub fn display_title(&self, max_len: usize) -> String {
        self.title.clone().unwrap_or_else(|| self.preview(max_len))
    }

    /// Calculate retrievability based on the forgetting curve
    ///
    /// Uses an exponential decay model: R = e^(-t/S)
    /// where t = time since last access (days), S = stability
    ///
    /// Returns a value between 0.0 and 1.0 representing how "recallable" this memory is
    pub fn retrievability(&self) -> f32 {
        let reference_time = self.last_accessed.unwrap_or(self.created);
        let days_since = Utc::now().signed_duration_since(reference_time).num_hours() as f32 / 24.0;

        // Stability affects how slowly the memory decays
        // Higher stability = slower decay
        let effective_stability = (self.stability * 100.0).max(1.0); // Scale to days

        // Exponential decay: R = e^(-t/S)
        (-days_since / effective_stability).exp()
    }

    /// Calculate a composite relevance score for ranking
    ///
    /// Combines:
    /// - Retrievability (forgetting curve)
    /// - Feedback score (helpful - harmful)
    /// - Access frequency
    pub fn relevance_score(&self) -> f32 {
        let retrievability = self.retrievability();

        // Feedback boost: +10% per net helpful vote (capped)
        let feedback_boost = 1.0 + (self.feedback_score() as f32 * 0.1).clamp(-0.5, 1.0);

        // Access count boost: logarithmic (diminishing returns)
        let access_boost = 1.0 + (self.access_count as f32 + 1.0).ln() * 0.1;

        retrievability * feedback_boost * access_boost
    }

    /// Reinforce the memory (call when accessed or marked helpful)
    ///
    /// Increases stability using spaced repetition principles:
    /// - Shorter intervals initially increase stability faster
    /// - Stability growth slows as it approaches 1.0
    pub fn reinforce(&mut self) {
        self.access_count += 1;
        self.last_accessed = Some(Utc::now());

        // Stability growth: S_new = S_old + (1 - S_old) * growth_rate
        // growth_rate depends on current retrievability
        let retrievability = self.retrievability();
        let growth_rate = 0.1 + (retrievability * 0.2); // 0.1-0.3 per reinforcement

        self.stability = (self.stability + (1.0 - self.stability) * growth_rate).min(1.0);
    }

    /// Apply decay to stability (call periodically or on prune operations)
    ///
    /// Only decreases stability if memory hasn't been accessed recently
    pub fn apply_decay(&mut self, days: f32) {
        // Only decay if not recently accessed
        if let Some(last) = self.last_accessed {
            let days_since = Utc::now().signed_duration_since(last).num_hours() as f32 / 24.0;

            if days_since > 7.0 {
                // Start decay after a week of no access
                let decay_rate = 0.02 * days; // 2% per day
                self.stability = (self.stability - decay_rate).max(0.1);
            }
        } else {
            // Never accessed - decay faster
            let decay_rate = 0.05 * days;
            self.stability = (self.stability - decay_rate).max(0.1);
        }
    }

    // =========================================================================
    // Hindsight-inspired opinion/belief methods
    // =========================================================================

    /// Check if this entry is an opinion (subjective belief)
    pub fn is_opinion(&self) -> bool {
        self.belief_type == BeliefType::Opinion
    }

    /// Check if this entry is a hypothesis (tentative belief)
    pub fn is_hypothesis(&self) -> bool {
        self.belief_type == BeliefType::Hypothesis
    }

    /// Check if this entry is a fact (objective)
    pub fn is_fact(&self) -> bool {
        self.belief_type == BeliefType::Fact
    }

    /// Reinforce confidence in an opinion/hypothesis (supporting evidence found)
    ///
    /// Implements Hindsight's opinion reinforcement:
    /// c' = min(c + α, 1.0)
    ///
    /// # Arguments
    /// * `strength` - How strong the supporting evidence is (0.0-1.0)
    pub fn reinforce_confidence(&mut self, strength: f32) {
        let alpha = 0.1 * strength.clamp(0.0, 1.0);
        self.confidence = (self.confidence + alpha).min(1.0);

        // If confidence is now high enough, promote hypothesis to opinion
        if self.belief_type == BeliefType::Hypothesis && self.confidence >= 0.7 {
            self.belief_type = BeliefType::Opinion;
        }
    }

    /// Weaken confidence in an opinion/hypothesis (contradicting evidence found)
    ///
    /// Implements Hindsight's opinion weakening:
    /// c' = max(c - α, 0.0)
    ///
    /// # Arguments
    /// * `strength` - How strong the contradicting evidence is (0.0-1.0)
    pub fn weaken_confidence(&mut self, strength: f32) {
        let alpha = 0.1 * strength.clamp(0.0, 1.0);
        self.confidence = (self.confidence - alpha).max(0.0);

        // If confidence drops too low, demote opinion to hypothesis
        if self.belief_type == BeliefType::Opinion && self.confidence < 0.3 {
            self.belief_type = BeliefType::Hypothesis;
        }
    }

    /// Strongly contradict an opinion (significant evidence against)
    ///
    /// Implements Hindsight's contradiction handling:
    /// c' = max(c - 2α, 0.0)
    ///
    /// # Arguments
    /// * `strength` - How strong the contradicting evidence is (0.0-1.0)
    pub fn contradict_confidence(&mut self, strength: f32) {
        let alpha = 0.2 * strength.clamp(0.0, 1.0);
        self.confidence = (self.confidence - alpha).max(0.0);

        // If confidence is very low, may want to archive this entry
        if self.confidence < 0.2 {
            self.belief_type = BeliefType::Hypothesis;
        }
    }

    /// Promote a hypothesis to an opinion (verified)
    pub fn promote_to_opinion(&mut self) {
        if self.belief_type == BeliefType::Hypothesis {
            self.belief_type = BeliefType::Opinion;
            self.confidence = self.confidence.max(0.6); // Ensure minimum confidence
        }
    }

    /// Promote an opinion to a fact (verified as objective)
    pub fn promote_to_fact(&mut self) {
        self.belief_type = BeliefType::Fact;
        self.confidence = 1.0;
    }

    /// Check if this memory should be pruned
    ///
    /// Candidates for pruning:
    /// - Low relevance score
    /// - Negative feedback
    /// - Very low stability + old
    pub fn should_prune(&self, relevance_threshold: f32) -> bool {
        // Never prune entries with positive feedback
        if self.feedback_score() > 0 {
            return false;
        }

        // Prune if negative feedback and low relevance
        if self.feedback_score() < 0 && self.relevance_score() < 0.3 {
            return true;
        }

        // Prune if very low relevance
        self.relevance_score() < relevance_threshold
    }
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            id: String::new(),
            scope: Scope::default(),
            entry_type: EntryType::default(),
            observation_type: None,
            source_ids: Vec::new(),
            tags: Vec::new(),
            created: Utc::now(),
            content: String::new(),
            raw_content: None,
            compressed: false,
            memory_tier: MemoryTier::Working,
            title: None,
            helpful_count: 0,
            harmful_count: 0,
            last_accessed: None,
            archived: false,
            session_id: None,
            source_tool: None,
            pending_extraction: false,
            pending_embedding: true,
            stability: default_stability(),
            access_count: 0,
            importance: default_importance(),
            valid_from: None,
            valid_until: None,
            review_after: None,
            last_reviewed: None,
            domain: None,
            belief_type: BeliefType::default(),
            confidence: default_confidence(),
            branch: None,
            team_id: None,
            share: None,
        }
    }
}

/// Infer observation type from source tool and content
fn infer_observation_type(source_tool: &str, content: &str) -> ObservationType {
    let content_lower = content.to_lowercase();

    // Check for specific patterns in content
    if content_lower.contains("fix") || content_lower.contains("bug") {
        return ObservationType::Bugfix;
    }
    if content_lower.contains("test") {
        return ObservationType::Test;
    }
    if content_lower.contains("refactor") {
        return ObservationType::Refactor;
    }
    if content_lower.contains("config")
        || content_lower.contains(".yaml")
        || content_lower.contains(".toml")
    {
        return ObservationType::Config;
    }

    // Infer from tool type
    match source_tool {
        "Write" | "Edit" => ObservationType::Change,
        "Bash" => {
            // Check bash command patterns
            if content_lower.contains("cargo test") || content_lower.contains("pytest") {
                ObservationType::Test
            } else if content_lower.contains("cargo build") || content_lower.contains("npm install")
            {
                ObservationType::Change
            } else {
                ObservationType::General
            }
        }
        "Read" => ObservationType::Discovery,
        _ => ObservationType::General,
    }
}
