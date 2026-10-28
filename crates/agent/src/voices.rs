use mediaa_core::transcript::Turn;

pub const SAMPLE_RATE: u32 = 16_000;
pub const VOICE_SECONDS: f64 = 30.0;
pub const VOICE_TURN_SECONDS: f64 = 10.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Voice {
    pub embedding: Vec<f32>,
    pub seconds: f64,
}

pub fn clips(samples: &[f32], turns: &[Turn], count: usize) -> Vec<Vec<f32>> {
    let mut audio = vec![Vec::<f32>::new(); count];
    for turn in turns {
        if turn.speaker as usize >= count {
            continue;
        }
        let taken = audio[turn.speaker as usize].len() as f64 / f64::from(SAMPLE_RATE);
        if taken >= VOICE_SECONDS {
            continue;
        }
        let low = (turn.start * f64::from(SAMPLE_RATE))
            .round()
            .clamp(0.0, samples.len() as f64) as usize;
        let high = (turn.end * f64::from(SAMPLE_RATE))
            .round()
            .clamp(0.0, samples.len() as f64) as usize;
        let limit =
            low + (VOICE_TURN_SECONDS.min(VOICE_SECONDS - taken) * f64::from(SAMPLE_RATE)) as usize;
        audio[turn.speaker as usize].extend_from_slice(&samples[low..high.max(low).min(limit)]);
    }
    audio
}

pub fn compact(turns: &mut [Turn]) -> usize {
    let mut ids = std::collections::HashMap::new();
    for turn in turns {
        let next = ids.len() as u32;
        turn.speaker = *ids.entry(turn.speaker).or_insert(next);
    }
    ids.len()
}

pub fn unit(vector: &[f32]) -> Option<Vec<f32>> {
    let norm = vector
        .iter()
        .map(|v| f64::from(*v) * f64::from(*v))
        .sum::<f64>()
        .sqrt();
    (norm > 1e-8).then(|| vector.iter().map(|v| (*v as f64 / norm) as f32).collect())
}

pub fn cosine(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return -1.0;
    }
    left.iter().zip(right).map(|(a, b)| a * b).sum()
}

struct Cluster {
    embedding: Option<Vec<f32>>,
    seconds: f64,
    members: Vec<usize>,
}

impl Cluster {
    fn new(index: usize, voice: &Voice) -> Cluster {
        Cluster {
            embedding: unit(&voice.embedding),
            seconds: voice.seconds,
            members: vec![index],
        }
    }

    fn merge(&mut self, other: Cluster) {
        let weight = self.seconds.max(0.0);
        let other_weight = other.seconds.max(0.0);
        if let (Some(mine), Some(theirs)) = (&self.embedding, &other.embedding) {
            let total = weight + other_weight;
            if total > 0.0 {
                let mixed: Vec<f32> = mine
                    .iter()
                    .zip(theirs)
                    .map(|(a, b)| {
                        ((f64::from(*a) * weight + f64::from(*b) * other_weight) / total) as f32
                    })
                    .collect();
                self.embedding = unit(&mixed);
            }
        }
        self.seconds += other.seconds;
        self.members.extend(other.members);
    }
}

pub fn merge_voices(
    turns: &[Turn],
    voices: &[Voice],
    threshold: f32,
    min_seconds: f64,
) -> (Vec<Turn>, Vec<Voice>) {
    merge(turns, voices, &[], Some(threshold), Some(min_seconds), None)
}

pub fn merge_voices_linked(
    turns: &[Turn],
    voices: &[Voice],
    links: &[(usize, usize)],
    threshold: f32,
    min_seconds: f64,
) -> (Vec<Turn>, Vec<Voice>) {
    merge(
        turns,
        voices,
        links,
        Some(threshold),
        Some(min_seconds),
        None,
    )
}

pub fn merge_voices_to(turns: &[Turn], voices: &[Voice], count: usize) -> (Vec<Turn>, Vec<Voice>) {
    merge(turns, voices, &[], None, None, Some(count))
}

fn merge(
    turns: &[Turn],
    voices: &[Voice],
    links: &[(usize, usize)],
    threshold: Option<f32>,
    min_seconds: Option<f64>,
    target: Option<usize>,
) -> (Vec<Turn>, Vec<Voice>) {
    let mut clusters: Vec<Cluster> = voices
        .iter()
        .enumerate()
        .map(|(index, voice)| Cluster::new(index, voice))
        .collect();
    let first_talk = |cluster: &Cluster| {
        cluster
            .members
            .iter()
            .filter_map(|&index| {
                turns
                    .iter()
                    .filter(|turn| turn.speaker as usize == index)
                    .map(|turn| turn.start)
                    .reduce(f64::min)
            })
            .reduce(f64::min)
            .unwrap_or(f64::INFINITY)
    };
    clusters.sort_by(|left, right| first_talk(left).total_cmp(&first_talk(right)));
    for &(a, b) in links {
        let left = clusters
            .iter()
            .position(|cluster| cluster.members.contains(&a));
        let right = clusters
            .iter()
            .position(|cluster| cluster.members.contains(&b));
        let (Some(left), Some(right)) = (left, right) else {
            continue;
        };
        if left == right {
            continue;
        }
        let other = clusters.remove(right.max(left));
        clusters[right.min(left)].merge(other);
    }
    if let Some(threshold) = threshold {
        loop {
            let mut best: Option<(f32, usize, usize)> = None;
            for left in 0..clusters.len() {
                for right in left + 1..clusters.len() {
                    let (Some(a), Some(b)) =
                        (&clusters[left].embedding, &clusters[right].embedding)
                    else {
                        continue;
                    };
                    let score = cosine(a, b);
                    if score >= threshold && best.is_none_or(|(best, _, _)| score > best) {
                        best = Some((score, left, right));
                    }
                }
            }
            let Some((_, left, right)) = best else { break };
            let other = clusters.remove(right);
            clusters[left].merge(other);
        }
    }
    if let Some(min_seconds) = min_seconds {
        while clusters.len() > 1 {
            let smallest = (0..clusters.len())
                .filter(|&index| {
                    clusters[index].seconds < min_seconds && clusters[index].embedding.is_some()
                })
                .min_by(|&a, &b| clusters[a].seconds.total_cmp(&clusters[b].seconds));
            let Some(smallest) = smallest else { break };
            let embedding = clusters[smallest]
                .embedding
                .clone()
                .expect("filtered by having one");
            let nearest = (0..clusters.len())
                .filter(|&index| index != smallest)
                .filter_map(|index| {
                    clusters[index]
                        .embedding
                        .as_ref()
                        .map(|other| (cosine(&embedding, other), index))
                })
                .max_by(|a, b| a.0.total_cmp(&b.0));
            let Some((_, nearest)) = nearest else { break };
            let other = clusters.remove(smallest);
            let target = if smallest < nearest {
                nearest - 1
            } else {
                nearest
            };
            clusters[target].merge(other);
        }
    }
    if let Some(target) = target {
        while clusters.len() > target {
            let mut best: Option<(f32, usize, usize)> = None;
            for left in 0..clusters.len() {
                for right in left + 1..clusters.len() {
                    let (Some(a), Some(b)) =
                        (&clusters[left].embedding, &clusters[right].embedding)
                    else {
                        continue;
                    };
                    let score = cosine(a, b);
                    if best.is_none_or(|(best, _, _)| score > best) {
                        best = Some((score, left, right));
                    }
                }
            }
            let Some((_, left, right)) = best else { break };
            let other = clusters.remove(right);
            clusters[left].merge(other);
        }
    }
    let mut order: Vec<usize> = (0..clusters.len()).collect();
    let first = |index: usize| {
        turns
            .iter()
            .filter(|t| clusters[index].members.contains(&(t.speaker as usize)))
            .map(|t| t.start)
            .fold(f64::INFINITY, f64::min)
    };
    order.sort_by(|&a, &b| first(a).total_cmp(&first(b)));
    let mut ids = vec![0u32; clusters.len()];
    for (new, &old) in order.iter().enumerate() {
        ids[old] = new as u32;
    }
    let name = |speaker: u32| {
        let cluster = clusters
            .iter()
            .position(|c| c.members.contains(&(speaker as usize)));
        cluster.map_or(speaker, |cluster| ids[cluster])
    };
    let turns = turns
        .iter()
        .map(|turn| Turn {
            speaker: name(turn.speaker),
            ..*turn
        })
        .collect::<Vec<_>>();
    let voices = order
        .iter()
        .map(|&index| Voice {
            embedding: clusters[index].embedding.clone().unwrap_or_default(),
            seconds: clusters[index].seconds,
        })
        .collect();
    (turns, voices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voices_merge_by_cosine_and_short_speakers_go_to_the_nearest() {
        let (a, b, c) = (
            unit(&[1.0, 0.0, 0.0]).unwrap(),
            unit(&[0.98, 0.2, 0.0]).unwrap(),
            unit(&[0.2, 0.0, 0.98]).unwrap(),
        );
        assert!(cosine(&a, &b) > 0.97);
        assert!(cosine(&a, &c) < 0.25);
        let voices = vec![
            Voice {
                embedding: a.clone(),
                seconds: 6.0,
            },
            Voice {
                embedding: b.clone(),
                seconds: 4.0,
            },
            Voice {
                embedding: c.clone(),
                seconds: 0.5,
            },
        ];
        let turns = vec![
            Turn {
                start: 0.0,
                end: 6.0,
                speaker: 0,
            },
            Turn {
                start: 6.0,
                end: 10.0,
                speaker: 1,
            },
            Turn {
                start: 10.0,
                end: 10.5,
                speaker: 2,
            },
        ];
        let (merged, found) = merge_voices(&turns, &voices, 0.9, 2.0);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(merged.iter().all(|turn| turn.speaker == 0));
        assert!((found[0].seconds - 10.5).abs() < 1e-9);

        let (merged, found) = merge_voices(&turns, &voices, 0.999, 2.0);
        assert_eq!(
            found.len(),
            2,
            "0 and 1 stay apart; the short 2 has no voice of its own: {found:?}"
        );
        assert_eq!(
            merged.iter().map(|turn| turn.speaker).collect::<Vec<_>>(),
            [0, 1, 0]
        );
        assert!(
            (merged[2].start - 10.0).abs() < 1e-9,
            "the turn of the short speaker is kept: {merged:?}"
        );

        let (merged, found) = merge_voices(&turns, &voices, 0.999, 0.0);
        assert_eq!(found.len(), 3);
        assert_eq!(
            merged.iter().map(|turn| turn.speaker).collect::<Vec<_>>(),
            [0, 1, 2]
        );
        assert!(found[2].seconds < 1.0);
    }

    #[test]
    fn merging_renumbers_the_speakers_in_the_order_they_talk() {
        let voices = vec![
            Voice {
                embedding: vec![1.0, 0.0],
                seconds: 5.0,
            },
            Voice {
                embedding: vec![0.0, 1.0],
                seconds: 5.0,
            },
        ];
        let turns = vec![
            Turn {
                start: 1.0,
                end: 6.0,
                speaker: 1,
            },
            Turn {
                start: 6.0,
                end: 11.0,
                speaker: 0,
            },
        ];
        let (merged, found) = merge_voices(&turns, &voices, 0.99, 0.0);
        assert_eq!(
            merged.iter().map(|turn| turn.speaker).collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].embedding, vec![0.0, 1.0]);
        let (merged, _) = merge_voices(&turns, &voices, 0.5, 0.0);
        assert_eq!(
            merged.iter().map(|turn| turn.speaker).collect::<Vec<_>>(),
            [0, 1]
        );
    }

    #[test]
    fn the_gaps_of_the_clustering_numbering_are_closed_in_the_order_speakers_talk() {
        let mut turns = vec![
            Turn {
                start: 0.0,
                end: 1.0,
                speaker: 9,
            },
            Turn {
                start: 1.0,
                end: 2.0,
                speaker: 4,
            },
            Turn {
                start: 2.0,
                end: 3.0,
                speaker: 11,
            },
            Turn {
                start: 3.0,
                end: 4.0,
                speaker: 9,
            },
        ];
        assert_eq!(compact(&mut turns), 3);
        assert_eq!(
            turns.iter().map(|turn| turn.speaker).collect::<Vec<_>>(),
            [0, 1, 2, 0]
        );
        let voices = vec![
            Voice {
                embedding: vec![1.0, 0.0],
                seconds: 2.0,
            },
            Voice {
                embedding: vec![0.0, 1.0],
                seconds: 1.0,
            },
            Voice {
                embedding: vec![0.5, 0.5],
                seconds: 1.0,
            },
        ];
        let (merged, found) = merge_voices(&turns, &voices, 0.99, 0.0);
        assert_eq!(found.len(), 3, "{found:?}");
        assert_eq!(
            merged.iter().map(|turn| turn.speaker).collect::<Vec<_>>(),
            [0, 1, 2, 0]
        );
    }

    #[test]
    fn merging_does_not_depend_on_how_the_clustering_numbered_the_speakers() {
        let (a, b, c) = (
            unit(&[1.0, 0.0, 0.0]).unwrap(),
            unit(&[0.97, 0.24, 0.0]).unwrap(),
            unit(&[0.0, 0.0, 1.0]).unwrap(),
        );
        let turns = vec![
            Turn {
                start: 0.0,
                end: 2.0,
                speaker: 0,
            },
            Turn {
                start: 2.0,
                end: 4.0,
                speaker: 1,
            },
            Turn {
                start: 4.0,
                end: 6.0,
                speaker: 2,
            },
            Turn {
                start: 6.0,
                end: 8.0,
                speaker: 1,
            },
        ];
        let voices = vec![
            Voice {
                embedding: a,
                seconds: 2.0,
            },
            Voice {
                embedding: b,
                seconds: 4.0,
            },
            Voice {
                embedding: c,
                seconds: 2.0,
            },
        ];
        let (merged, found) = merge_voices(&turns, &voices, 0.9, 2.0);
        assert_eq!(
            merged.iter().map(|turn| turn.speaker).collect::<Vec<_>>(),
            [0, 0, 1, 0],
            "{merged:?}"
        );
        assert_eq!(found.len(), 2);
        let swapped = turns
            .iter()
            .map(|turn| Turn {
                speaker: [2, 1, 0][turn.speaker as usize],
                ..*turn
            })
            .collect::<Vec<_>>();
        let swapped_voices = vec![voices[2].clone(), voices[1].clone(), voices[0].clone()];
        let (also, also_found) = merge_voices(&swapped, &swapped_voices, 0.9, 2.0);
        assert_eq!(also, merged);
        assert_eq!(also_found.len(), found.len());
        for (left, right) in found.iter().zip(&also_found) {
            assert!((left.seconds - right.seconds).abs() < 1e-9);
            assert!(left
                .embedding
                .iter()
                .zip(&right.embedding)
                .all(|(l, r)| (l - r).abs() < 1e-6));
        }
    }

    #[test]
    fn linked_voices_merge_whatever_their_cosine_says() {
        let turns = vec![
            Turn {
                start: 0.0,
                end: 5.0,
                speaker: 0,
            },
            Turn {
                start: 5.0,
                end: 10.0,
                speaker: 1,
            },
        ];
        let voices = vec![
            Voice {
                embedding: vec![1.0, 0.0],
                seconds: 5.0,
            },
            Voice {
                embedding: vec![0.0, 1.0],
                seconds: 5.0,
            },
        ];
        let (merged, found) = merge_voices_linked(&turns, &voices, &[(0, 1)], 0.8, 0.0);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(merged.iter().all(|turn| turn.speaker == 0), "{merged:?}");
        assert!((found[0].seconds - 10.0).abs() < 1e-9);
        let (_, found) = merge_voices_linked(&turns, &voices, &[(7, 9)], 0.8, 0.0);
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn merging_to_a_known_count_takes_the_closest_voices_first() {
        let (a, b, c) = (
            unit(&[1.0, 0.0]).unwrap(),
            unit(&[0.9, 0.1]).unwrap(),
            unit(&[0.0, 1.0]).unwrap(),
        );
        let voices = vec![
            Voice {
                embedding: a,
                seconds: 2.0,
            },
            Voice {
                embedding: b,
                seconds: 2.0,
            },
            Voice {
                embedding: c,
                seconds: 4.0,
            },
        ];
        let turns = vec![
            Turn {
                start: 0.0,
                end: 2.0,
                speaker: 0,
            },
            Turn {
                start: 2.0,
                end: 4.0,
                speaker: 1,
            },
            Turn {
                start: 4.0,
                end: 8.0,
                speaker: 2,
            },
        ];
        let (merged, found) = merge_voices_to(&turns, &voices, 2);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(
            merged.iter().map(|turn| turn.speaker).collect::<Vec<_>>(),
            [0, 0, 1],
            "a and b are one speaker"
        );
        assert!((found[0].seconds - 4.0).abs() < 1e-9 && (found[1].seconds - 4.0).abs() < 1e-9);

        let (_, found) = merge_voices_to(&turns, &voices, 3);
        assert_eq!(found.len(), 3);
        let (_, found) = merge_voices_to(&turns, &voices, 9);
        assert_eq!(found.len(), 3);
    }
}
