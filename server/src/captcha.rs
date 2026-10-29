//! The login captcha: digits drawn as jittered strokes in an SVG among noise
//! lines. The image holds no text to copy, and each challenge is good for one
//! guess within [`LIFETIME`], so guessing the password costs a solved picture
//! per try.

use std::collections::HashMap;
use std::fmt::Write;
use std::time::{Duration, Instant};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

pub const LIFETIME: Duration = Duration::from_secs(300);
/// Open challenges kept at most; the oldest go first.
const CAPACITY: usize = 1000;
const LENGTH: usize = 5;
const WIDTH: f32 = 200.0;
const HEIGHT: f32 = 64.0;

/// Digit strokes on a 4×6 grid, y down; each digit is one or two polylines.
const GLYPHS: [&[&[(f32, f32)]]; 10] = [
    &[&[(1.0, 0.0), (3.0, 0.0), (4.0, 1.0), (4.0, 5.0), (3.0, 6.0), (1.0, 6.0), (0.0, 5.0), (0.0, 1.0), (1.0, 0.0)]],
    &[&[(1.0, 1.0), (2.0, 0.0), (2.0, 6.0)], &[(1.0, 6.0), (3.0, 6.0)]],
    &[&[(0.0, 1.0), (1.0, 0.0), (3.0, 0.0), (4.0, 1.0), (4.0, 2.0), (0.0, 6.0), (4.0, 6.0)]],
    &[&[(0.0, 0.0), (4.0, 0.0), (2.0, 2.5), (3.0, 2.5), (4.0, 3.5), (4.0, 5.0), (3.0, 6.0), (1.0, 6.0), (0.0, 5.0)]],
    &[&[(3.0, 6.0), (3.0, 0.0), (0.0, 4.0), (4.0, 4.0)]],
    &[&[(4.0, 0.0), (0.0, 0.0), (0.0, 2.5), (3.0, 2.5), (4.0, 3.5), (4.0, 5.0), (3.0, 6.0), (0.0, 6.0)]],
    &[&[(3.0, 0.0), (1.0, 0.0), (0.0, 1.0), (0.0, 5.0), (1.0, 6.0), (3.0, 6.0), (4.0, 5.0), (4.0, 3.5), (3.0, 2.5), (0.0, 2.5)]],
    &[&[(0.0, 0.0), (4.0, 0.0), (1.5, 6.0)]],
    &[
        &[(1.0, 0.0), (3.0, 0.0), (4.0, 1.0), (4.0, 2.0), (3.0, 3.0), (1.0, 3.0), (0.0, 2.0), (0.0, 1.0), (1.0, 0.0)],
        &[(1.0, 3.0), (0.0, 4.0), (0.0, 5.0), (1.0, 6.0), (3.0, 6.0), (4.0, 5.0), (4.0, 4.0), (3.0, 3.0)],
    ],
    &[&[(4.0, 3.5), (1.0, 3.5), (0.0, 2.5), (0.0, 1.0), (1.0, 0.0), (3.0, 0.0), (4.0, 1.0), (4.0, 5.0), (3.0, 6.0), (1.0, 6.0)]],
];

struct Challenge {
    answer: String,
    /// Seeds the drawing, so the picture is the same on every fetch.
    seed: u64,
    made: Instant,
}

#[derive(Default)]
pub struct Challenges {
    open: HashMap<String, Challenge>,
}

impl Challenges {
    /// A new challenge; returns its id.
    pub fn issue(&mut self, rng: &mut impl Rng, now: Instant) -> String {
        self.open.retain(|_, c| now.duration_since(c.made) < LIFETIME);
        while self.open.len() >= CAPACITY {
            let Some(oldest) = self.open.iter().min_by_key(|(_, c)| c.made).map(|(id, _)| id.clone()) else { break };
            self.open.remove(&oldest);
        }
        let id: String = (0..16).map(|_| format!("{:02x}", rng.random::<u8>())).collect();
        let answer = (0..LENGTH).map(|_| char::from(b'0' + rng.random_range(0..10u8))).collect();
        self.open.insert(id.clone(), Challenge { answer, seed: rng.random(), made: now });
        id
    }

    /// The picture for an open challenge.
    pub fn image(&self, id: &str) -> Option<String> {
        self.open.get(id).map(|c| draw(&c.answer, c.seed))
    }

    pub fn answer(&self, id: &str) -> Option<String> {
        self.open.get(id).map(|c| c.answer.clone())
    }

    /// Whether `guess` solves the challenge; right or wrong, it is used up.
    pub fn check(&mut self, id: &str, guess: &str, now: Instant) -> bool {
        let guess: String = guess.chars().filter(|c| !c.is_whitespace()).collect();
        self.open
            .remove(id)
            .is_some_and(|c| now.duration_since(c.made) < LIFETIME && c.answer == guess)
    }
}

/// The answer's digits, each turned, scaled and shaken, among noise strokes.
fn draw(answer: &str, seed: u64) -> String {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 {WIDTH} {HEIGHT}' width='{WIDTH}' height='{HEIGHT}'>\
         <g fill='none' stroke='#6E7781' stroke-linecap='round' stroke-linejoin='round'>"
    );
    let cell = WIDTH / (LENGTH as f32 + 1.0);
    for (i, digit) in answer.bytes().enumerate() {
        let angle: f32 = rng.random_range(-0.35..0.35);
        let scale = 6.0 * rng.random_range(0.8..1.15);
        let cx = cell * (i as f32 + 1.0) + rng.random_range(-4.0..4.0);
        let cy = HEIGHT / 2.0 + rng.random_range(-6.0..6.0);
        let (sin, cos) = angle.sin_cos();
        let mut d = String::new();
        for stroke in GLYPHS[usize::from(digit - b'0')] {
            for (n, &(x, y)) in stroke.iter().enumerate() {
                let x = (x - 2.0 + rng.random_range(-0.2..0.2)) * scale;
                let y = (y - 3.0 + rng.random_range(-0.2..0.2)) * scale;
                let _ = write!(d, "{}{:.1} {:.1}", if n == 0 { 'M' } else { 'L' }, cx + x * cos - y * sin, cy + x * sin + y * cos);
            }
        }
        let _ = write!(svg, "<path stroke-width='3' d='{d}'/>");
    }
    for _ in 0..5 {
        let mut point = || (rng.random_range(0.0..WIDTH), rng.random_range(0.0..HEIGHT));
        let (a, b, c) = (point(), point(), point());
        let width = rng.random_range(1.0..1.8);
        let _ = write!(svg, "<path stroke-width='{width:.1}' d='M{:.1} {:.1}Q{:.1} {:.1} {:.1} {:.1}'/>", a.0, a.1, b.0, b.1, c.0, c.1);
    }
    svg + "</g></svg>"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issued(challenges: &mut Challenges, now: Instant) -> (String, String) {
        let id = challenges.issue(&mut rand::rng(), now);
        let answer = challenges.answer(&id).unwrap();
        (id, answer)
    }

    #[test]
    fn a_late_answer_fails() {
        let mut challenges = Challenges::default();
        let start = Instant::now();
        let (id, answer) = issued(&mut challenges, start);
        assert!(!challenges.check(&id, &answer, start + LIFETIME));
        let (id, answer) = issued(&mut challenges, start);
        assert!(challenges.check(&id, &format!(" {answer} "), start + LIFETIME - Duration::from_secs(1)));
    }

    #[test]
    fn open_challenges_are_bounded_and_the_oldest_go() {
        let mut challenges = Challenges::default();
        let start = Instant::now();
        let (first, answer) = issued(&mut challenges, start);
        for n in 1..=CAPACITY as u64 {
            challenges.issue(&mut rand::rng(), start + Duration::from_millis(n));
        }
        assert_eq!(challenges.open.len(), CAPACITY);
        assert!(!challenges.check(&first, &answer, start + Duration::from_secs(1)));
    }

    #[test]
    fn the_picture_is_stable_and_differs_per_challenge() {
        let mut challenges = Challenges::default();
        let now = Instant::now();
        let a = challenges.issue(&mut rand::rng(), now);
        let b = challenges.issue(&mut rand::rng(), now);
        assert_eq!(challenges.image(&a), challenges.image(&a));
        assert_ne!(challenges.image(&a), challenges.image(&b));
        // Every digit is drawable.
        for digit in 0..10u8 {
            assert!(draw(&char::from(b'0' + digit).to_string().repeat(LENGTH), 1).contains("<path"));
        }
    }
}
