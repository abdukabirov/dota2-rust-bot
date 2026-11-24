use crate::{domain::r#match::Match, providers::opendota::MatchProvider};
use anyhow::Result;

#[derive(Clone)]
pub struct MatchService<P> {
    provider: P,
}

impl<P: MatchProvider> MatchService<P> {
    pub fn new(provider: P) -> Self {
        Self { provider }
    }
    pub async fn live(&self) -> Result<Vec<Match>> {
        self.provider.live_matches().await
    }
    pub async fn recent(&self) -> Result<Vec<Match>> {
        self.provider.recent_matches().await
    }
    pub async fn get_match(&self, id: i64) -> Result<Match> {
        self.provider.get_match(id).await
    }
    pub async fn tournaments(&self) -> Result<Vec<crate::domain::r#match::Tournament>> {
        self.provider.tournaments().await
    }
    pub async fn tournament_matches(&self, league_id: i64) -> Result<Vec<Match>> {
        self.provider.tournament_matches(league_id).await
    }
}

pub fn format_match(m: &Match) -> String {
    format!(
        "⚔️ {} — {}\n{}:{}\n🏆 {}\n🎯 пики: {}, баны: {}",
        m.radiant.name,
        m.dire.name,
        m.radiant.score,
        m.dire.score,
        m.tournament.as_deref().unwrap_or("без турнира"),
        m.draft.iter().filter(|item| item.is_pick).count(),
        m.draft.iter().filter(|item| !item.is_pick).count()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::r#match::{DraftAction, Team};
    #[test]
    fn formats_draft_counts() {
        let value = Match {
            radiant: Team {
                name: "R".into(),
                score: 1,
                ..Default::default()
            },
            dire: Team {
                name: "D".into(),
                score: 2,
                ..Default::default()
            },
            draft: vec![
                DraftAction {
                    is_pick: true,
                    ..Default::default()
                },
                DraftAction {
                    is_pick: false,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert!(format_match(&value).contains("пики: 1, баны: 1"));
    }
}
