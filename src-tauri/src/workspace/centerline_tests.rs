use super::{extract, BrushGuide, ExtractionError};
use crate::{frame::FrameImage, recipe::Polarity};

#[test]
fn equally_plausible_branches_are_rejected_even_when_their_tails_merge() {
    let (width, height) = (200, 100);
    for merge in [false, true] {
        let pixels = (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| {
                    let offset = if merge {
                        9.0 * (1.0 - ((x as f32 - 80.0) / 70.0).clamp(0.0, 1.0))
                    } else {
                        9.0
                    };
                    if (y as f32 - 50.0 - offset)
                        .abs()
                        .min((y as f32 - 50.0 + offset).abs())
                        <= 1.5
                    {
                        30
                    } else {
                        220
                    }
                })
            })
            .collect();
        let image = FrameImage::new(width, height, pixels);
        for reverse in [false, true] {
            let mut guide = BrushGuide {
                points: vec![[20.0, 50.0], [175.0, 50.0]],
                radius_px: 20.0,
            };
            if reverse {
                guide.points.reverse();
            }
            let result = extract(
                &image,
                [0, 0, width, height],
                Polarity::Dark,
                Some(&guide),
                10.0,
            );
            assert!(
                matches!(result, Err(ExtractionError::Failed(ref failure)) if failure.reason == "ambiguous"),
                "merge={merge}, reverse={reverse}: {result:?}"
            );
            let mut focused = BrushGuide {
                points: if merge {
                    vec![[20.0, 41.0], [80.0, 41.0], [150.0, 50.0], [175.0, 50.0]]
                } else {
                    vec![[20.0, 41.0], [175.0, 41.0]]
                },
                radius_px: 20.0,
            };
            if reverse {
                focused.points.reverse();
            }
            let path = extract(
                &image,
                [0, 0, width, height],
                Polarity::Dark,
                Some(&focused),
                10.0,
            )
            .unwrap()
            .path;
            let branch: Vec<_> = path.iter().filter(|p| p[0] < 75.0).collect();
            assert!(branch.len() >= 2);
            assert!(branch.iter().all(|p| (p[1] - 41.0).abs() < 1.0));
        }
    }
}

#[test]
fn widening_occlusion_with_a_sharp_turn_is_trimmed_at_the_end_but_not_bridged_in_the_middle() {
    let (width, height) = (240, 160);
    for middle in [false, true] {
        let pixels = (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| {
                    let occluded = if middle {
                        (100..125).contains(&x)
                    } else {
                        x >= 150
                    };
                    let center = if occluded { 66 } else { 60 };
                    let half = if occluded { 13 } else { 10 };
                    if (y as i32 - center).abs() <= half {
                        30
                    } else {
                        220
                    }
                })
            })
            .collect();
        let image = FrameImage::new(width, height, pixels);
        let mut endpoints = Vec::new();
        for reverse in [false, true] {
            let mut guide = BrushGuide {
                points: vec![[20.0, 60.0], [185.0, 60.0]],
                radius_px: 30.0,
            };
            if reverse {
                guide.points.reverse();
            }
            let result = extract(
                &image,
                [0, 0, width, height],
                Polarity::Dark,
                Some(&guide),
                10.0,
            );
            if middle {
                assert!(
                    matches!(result, Err(ExtractionError::Failed(ref f)) if f.reason == "discontinuity"),
                    "{result:?}"
                );
            } else {
                let extraction = result.unwrap();
                let last = extraction
                    .path
                    .iter()
                    .max_by(|a, b| a[0].total_cmp(&b[0]))
                    .unwrap();
                assert!((145.0..150.0).contains(&last[0]), "{last:?}");
                assert!((last[1] - 60.0).abs() < 1.0);
                assert!(extraction.notice.is_some());
                endpoints.push(*last);
            }
        }
        if !middle {
            assert!((endpoints[0][0] - endpoints[1][0]).abs() <= 3.0);
        }
    }

    for reflection_end in [None, Some(140), Some(width)] {
        let pixels = (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| {
                    let half = if reflection_end.is_none() && x >= 150 {
                        13
                    } else {
                        10
                    };
                    let in_bead = (y as i32 - 60).abs() <= half;
                    let reflection = reflection_end.is_some_and(|end| (85..end).contains(&x))
                        && (y as i32 - 60).abs() < 2;
                    if in_bead && !reflection {
                        30
                    } else {
                        220
                    }
                })
            })
            .collect();
        let image = FrameImage::new(width, height, pixels);
        let guide = BrushGuide {
            points: vec![[20.0, 60.0], [195.0, 60.0]],
            radius_px: 30.0,
        };
        let extraction = extract(
            &image,
            [0, 0, width, height],
            Polarity::Dark,
            Some(&guide),
            10.0,
        )
        .unwrap();
        assert!(extraction.path.first().unwrap()[0] <= 21.0);
        assert!(extraction.path.last().unwrap()[0] >= 194.0);
        assert!(extraction.path.iter().all(|p| (p[1] - 60.0).abs() < 1.0));
    }

    let mut curve = vec![[20.0f32, 60.0], [130.0, 60.0]];
    for i in 1..=15 {
        let angle = (i as f32 * 6.0 - 90.0).to_radians();
        curve.push([130.0 + 30.0 * angle.cos(), 90.0 + 30.0 * angle.sin()]);
    }
    curve.push([160.0, 120.0]);
    let pixels = (0..height)
        .flat_map(|y| {
            (0..width)
                .map(|x| {
                    let on = curve.windows(2).any(|s| {
                        let d = [s[1][0] - s[0][0], s[1][1] - s[0][1]];
                        let t = (((x as f32 - s[0][0]) * d[0] + (y as f32 - s[0][1]) * d[1])
                            / (d[0] * d[0] + d[1] * d[1]))
                            .clamp(0.0, 1.0);
                        (x as f32 - s[0][0] - t * d[0]).hypot(y as f32 - s[0][1] - t * d[1]) <= 10.0
                    });
                    if on {
                        30
                    } else {
                        220
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let image = FrameImage::new(width, height, pixels);
    for reverse in [false, true] {
        let mut guide = BrushGuide {
            points: curve.clone(),
            radius_px: 30.0,
        };
        if reverse {
            guide.points.reverse();
        }
        let extraction = extract(
            &image,
            [0, 0, width, height],
            Polarity::Dark,
            Some(&guide),
            10.0,
        )
        .unwrap();
        for expected in [[20.0, 60.0], [160.0, 120.0]] {
            assert!(extraction
                .path
                .iter()
                .any(|p| (p[0] - expected[0]).hypot(p[1] - expected[1]) < 1.0));
        }
    }
}

#[test]
fn a_new_occlusion_after_end_trimming_is_cut_only_within_the_original_end_budget() {
    let (width, height) = (240, 160);
    for occlusion_end in [50, 72] {
        let pixels = (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| {
                    let center = if x < occlusion_end { 66 } else { 60 };
                    let half = if x < 35 {
                        25
                    } else if x < occlusion_end {
                        13
                    } else {
                        10
                    };
                    if (y as i32 - center).abs() <= half {
                        30
                    } else {
                        220
                    }
                })
            })
            .collect();
        let image = FrameImage::new(width, height, pixels);
        let mut boundaries = Vec::new();
        for reverse in [false, true] {
            let mut guide = BrushGuide {
                points: vec![[20.0, 60.0], [200.0, 60.0]],
                radius_px: 35.0,
            };
            if reverse {
                guide.points.reverse();
            }
            let result = extract(
                &image,
                [0, 0, width, height],
                Polarity::Dark,
                Some(&guide),
                10.0,
            );
            if occlusion_end == 72 {
                assert!(
                    matches!(result, Err(ExtractionError::Failed(ref f)) if f.reason == "discontinuity"),
                    "{result:?}"
                );
            } else {
                let extraction = result.unwrap();
                let boundary = extraction
                    .path
                    .iter()
                    .min_by(|a, b| a[0].total_cmp(&b[0]))
                    .unwrap();
                assert!((50.0..54.0).contains(&boundary[0]), "{boundary:?}");
                assert!(extraction.path.iter().all(|p| (p[1] - 60.0).abs() < 1.0));
                assert!(extraction.path.iter().any(|p| p[0] >= 199.0));
                assert!(extraction.notice.is_some());
                boundaries.push(boundary[0]);
            }
        }
        if occlusion_end == 50 {
            assert!((boundaries[0] - boundaries[1]).abs() <= 3.0);
        }
    }
}
