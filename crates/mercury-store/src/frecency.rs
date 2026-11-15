/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! Frecency: a page's rank for address-bar suggestions, from how often and how recently it was
//! visited, how it was reached, and whether it is bookmarked.
//!
//! Ported from Mozilla application-services, <https://github.com/mozilla/application-services>,
//! commit `5f95eb6bafa36c4a86ff3109dacbf9026029833f`, `components/places/src/frecency.rs`,
//! MPL-2.0. The settings, the bucket weights, the transition bonuses, the sampling of the most
//! recent visits and the final formula (including its `f32` arithmetic and rounding) are the
//! donor's. Deliberate differences:
//! - "now" is the caller's `now_ms` instead of `Timestamp::now()`, so the store never reads the
//!   clock.
//! - No redirect handling: Mercury does not record redirect visits or `from_visit`, so the
//!   redirect-source bonus and the redirect-target join are left out, as are the bonuses for
//!   visit types Mercury never records (embed, framed link, redirects, download).
//! - No `place:` query URLs, so the donor's `is_query` case is left out.
//! - The donor's `foreign_count` (references from bookmarks and other tables) is the number of
//!   bookmarks pointing at the page.
//! - The donor's unvisited-bookmark score adds the unvisited-typed bonus when the page's `typed`
//!   flag is set. Mercury's `typed_count` is recomputed from the visit rows, so it is 0 whenever
//!   there are no visits and that branch is left out (see `compute_unvisited_bookmark_frecency`).

use rusqlite::{Connection, params};

use crate::error::Result;
use crate::history::VisitTransition;

/// The tunable constants of the calculation. Field names and defaults are the donor's, which in
/// turn mirror Firefox's `places.frecency.*` preferences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrecencySettings {
    pub(crate) num_visits: i32,
    pub(crate) first_bucket_cutoff_days: i32,
    pub(crate) second_bucket_cutoff_days: i32,
    pub(crate) third_bucket_cutoff_days: i32,
    pub(crate) fourth_bucket_cutoff_days: i32,
    pub(crate) first_bucket_weight: i32,
    pub(crate) second_bucket_weight: i32,
    pub(crate) third_bucket_weight: i32,
    pub(crate) fourth_bucket_weight: i32,
    pub(crate) default_bucket_weight: i32,
    pub(crate) link_visit_bonus: i32,
    pub(crate) typed_visit_bonus: i32,
    pub(crate) bookmark_visit_bonus: i32,
    pub(crate) default_visit_bonus: i32,
    pub(crate) unvisited_bookmark_bonus: i32,
    pub(crate) unvisited_typed_bonus: i32,
    pub(crate) reload_visit_bonus: i32,
}

/// The donor's defaults (`DEFAULT_FRECENCY_SETTINGS`).
pub(crate) const DEFAULT_FRECENCY_SETTINGS: FrecencySettings = FrecencySettings {
    num_visits: 10,
    first_bucket_cutoff_days: 4,
    second_bucket_cutoff_days: 14,
    third_bucket_cutoff_days: 31,
    fourth_bucket_cutoff_days: 90,
    first_bucket_weight: 100,
    second_bucket_weight: 70,
    third_bucket_weight: 50,
    fourth_bucket_weight: 30,
    default_bucket_weight: 10,
    link_visit_bonus: 100,
    typed_visit_bonus: 2000,
    bookmark_visit_bonus: 75,
    default_visit_bonus: 0,
    unvisited_bookmark_bonus: 140,
    unvisited_typed_bonus: 200,
    reload_visit_bonus: 0,
};

impl FrecencySettings {
    /// The bonus for one visit of this type. `visited` is false when scoring a page that has no
    /// visits (an unvisited bookmark).
    pub(crate) fn get_transition_bonus(
        &self,
        visit_type: Option<VisitTransition>,
        visited: bool,
    ) -> i32 {
        match (visit_type, visited) {
            (Some(VisitTransition::Link), _) => self.link_visit_bonus,
            (Some(VisitTransition::Reload), _) => self.reload_visit_bonus,
            (Some(VisitTransition::Typed), true) => self.typed_visit_bonus,
            (Some(VisitTransition::Typed), false) => self.unvisited_typed_bonus,
            (Some(VisitTransition::Bookmark), true) => self.bookmark_visit_bonus,
            (Some(VisitTransition::Bookmark), false) => self.unvisited_bookmark_bonus,
            (None, _) => self.default_visit_bonus,
        }
    }

    /// The weight of a visit `age_in_days` old.
    pub(crate) fn get_frecency_aged_weight(&self, age_in_days: i32) -> i32 {
        if age_in_days <= self.first_bucket_cutoff_days {
            self.first_bucket_weight
        } else if age_in_days <= self.second_bucket_cutoff_days {
            self.second_bucket_weight
        } else if age_in_days <= self.third_bucket_cutoff_days {
            self.third_bucket_weight
        } else if age_in_days <= self.fourth_bucket_cutoff_days {
            self.fourth_bucket_weight
        } else {
            self.default_bucket_weight
        }
    }
}

struct FrecencyComputation<'db, 's> {
    conn: &'db Connection,
    settings: &'s FrecencySettings,
    page_id: i64,
    now_ms: i64,
    visit_count: i64,
    foreign_count: i64,
}

impl<'db, 's> FrecencyComputation<'db, 's> {
    fn new(
        conn: &'db Connection,
        settings: &'s FrecencySettings,
        page_id: i64,
        now_ms: i64,
    ) -> Result<Self> {
        let mut stmt = conn.prepare_cached(
            "SELECT visit_count, (SELECT count(*) FROM bookmarks WHERE place_id = places.id)
             FROM places WHERE id = ?1",
        )?;
        let (visit_count, foreign_count) =
            stmt.query_row([page_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(Self {
            conn,
            settings,
            page_id,
            now_ms,
            visit_count,
            foreign_count,
        })
    }

    fn calculate(&self) -> Result<i64> {
        let (num_sampled_visits, sample_score) = if self.visit_count > 0 {
            self.score_recent_visits()?
        } else {
            (0, 0.0f32)
        };
        Ok(i64::from(if num_sampled_visits > 0 {
            self.get_frecency_for_sample(num_sampled_visits, sample_score)
        } else if !self.has_bookmark() {
            0
        } else {
            // Unvisited bookmarks get a non-zero score so that they show up in suggestions.
            self.compute_unvisited_bookmark_frecency()
        }))
    }

    fn has_bookmark(&self) -> bool {
        self.foreign_count > 0
    }

    /// Scores a sample of the most recent visits. Returns the number sampled and their points.
    fn score_recent_visits(&self) -> Result<(usize, f32)> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT transition, visited_at FROM visits WHERE place_id = ?1
             ORDER BY visited_at DESC, id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![self.page_id, self.settings.num_visits], |row| {
            let transition: i64 = row.get(0)?;
            let visited_at: i64 = row.get(1)?;
            let age_in_days = (self.now_ms as f64 - visited_at as f64) / 86_400_000.0;
            Ok((
                VisitTransition::from_i64(transition),
                age_in_days.round() as i32,
            ))
        })?;

        let mut num_sampled_visits = 0;
        let mut points_for_sampled_visits = 0.0f32;
        for row in rows {
            let (visit_type, age_in_days) = row?;
            let mut bonus = self.settings.get_transition_bonus(visit_type, true);
            if self.has_bookmark() {
                bonus += self
                    .settings
                    .get_transition_bonus(Some(VisitTransition::Bookmark), true);
            }
            if bonus != 0 {
                let weight = self.settings.get_frecency_aged_weight(age_in_days) as f32;
                points_for_sampled_visits += weight * (bonus as f32 / 100.0);
            }
            num_sampled_visits += 1;
        }
        Ok((num_sampled_visits, points_for_sampled_visits))
    }

    fn get_frecency_for_sample(&self, num_sampled: usize, score: f32) -> i32 {
        if score == 0.0f32 {
            // Every sampled visit had a zero bonus. The page has visits, so keep it visible in
            // suggestions with -1 rather than 0.
            -1
        } else {
            // ceil() so that a page is never rounded down to 0.
            ((self.visit_count as f32) * score.ceil() / (num_sampled as f32)).ceil() as i32
        }
    }

    fn compute_unvisited_bookmark_frecency(&self) -> i32 {
        // The donor adds the unvisited-typed bonus here when the page's `typed` flag is set.
        // Mercury's `typed_count` is recomputed from the visit rows, so a page without visits
        // always has 0 and that branch could never run; it is left out.
        let bonus = self
            .settings
            .get_transition_bonus(Some(VisitTransition::Bookmark), false);
        // Assume the visit is "now", so use the first bucket.
        let score = (self.settings.first_bucket_weight as f32) * (bonus as f32 / 100.0f32);
        score.ceil() as i32
    }
}

/// The frecency of a page as of `now_ms`.
pub(crate) fn calculate_frecency(
    conn: &Connection,
    settings: &FrecencySettings,
    page_id: i64,
    now_ms: i64,
) -> Result<i64> {
    let fc = FrecencyComputation::new(conn, settings, page_id, now_ms)?;
    fc.calculate()
}

/// [`calculate_frecency`] for a page whose visit and bookmark counts the caller has already
/// read, saving a query.
pub(crate) fn calculate_frecency_from_counts(
    conn: &Connection,
    settings: &FrecencySettings,
    page_id: i64,
    visit_count: i64,
    bookmark_count: i64,
    now_ms: i64,
) -> Result<i64> {
    FrecencyComputation {
        conn,
        settings,
        page_id,
        now_ms,
        visit_count,
        foreign_count: bookmark_count,
    }
    .calculate()
}

/// Recomputes a page's stored frecency as of `now_ms`, and records that time.
pub(crate) fn update_frecency(conn: &Connection, page_id: i64, now_ms: i64) -> Result<()> {
    let frecency = calculate_frecency(conn, &DEFAULT_FRECENCY_SETTINGS, page_id, now_ms)?;
    conn.prepare_cached("UPDATE places SET frecency = ?1, frecency_at = ?3 WHERE id = ?2")?
        .execute(params![frecency, page_id, now_ms])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_boundaries_are_inclusive() {
        let s = DEFAULT_FRECENCY_SETTINGS;
        let weights: Vec<i32> = [-3, 0, 4, 5, 14, 15, 31, 32, 90, 91, 10_000]
            .into_iter()
            .map(|days| s.get_frecency_aged_weight(days))
            .collect();
        assert_eq!(weights, [100, 100, 100, 70, 70, 50, 50, 30, 30, 10, 10]);
    }

    #[test]
    fn transition_bonuses_match_the_donor_defaults() {
        let s = DEFAULT_FRECENCY_SETTINGS;
        assert_eq!(
            s.get_transition_bonus(Some(VisitTransition::Link), true),
            100
        );
        assert_eq!(
            s.get_transition_bonus(Some(VisitTransition::Typed), true),
            2000
        );
        assert_eq!(
            s.get_transition_bonus(Some(VisitTransition::Typed), false),
            200
        );
        assert_eq!(
            s.get_transition_bonus(Some(VisitTransition::Bookmark), true),
            75
        );
        assert_eq!(
            s.get_transition_bonus(Some(VisitTransition::Bookmark), false),
            140
        );
        assert_eq!(
            s.get_transition_bonus(Some(VisitTransition::Reload), true),
            0
        );
        assert_eq!(s.get_transition_bonus(None, true), 0);
    }
}
