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
