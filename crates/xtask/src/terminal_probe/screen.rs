//! Validate the rendered terminal, including cursor movement and row footprint.
use anyhow::{Context, Result, ensure};
use std::collections::BTreeSet;

pub fn check(path: &str) -> Result<()> {
    let bytes = std::fs::read(path).context("read raw terminal recording")?;
    verify(&bytes)?;
    let first_prompt = bytes
        .windows(b"alpha authentication".len())
        .position(|w| w == b"alpha authentication")
        .context("missing handoff marker")?;
    ensure!(
        verify(&bytes[..first_prompt]).is_err(),
        "truncated recording falsely passed"
    );
    let native = bytes
        .windows(b"Synthetic token: ".len())
        .position(|w| w == b"Synthetic token: ")
        .context("missing native prompt")?
        + b"Synthetic token: ".len();
    let mut contaminated = bytes[..native].to_vec();
    contaminated
        .extend_from_slice("\r\n[██░░░░░░] github.com/demo/alpha Connecting\r\n".as_bytes());
    contaminated.extend_from_slice(&bytes[native..]);
    ensure!(
        verify(&contaminated).is_err(),
        "redraw over native prompt falsely passed"
    );
    println!(
        "PASS screen: fixed rows, repeated full traversals, ordered native handoffs, resumed work, final cleanup"
    );
    println!(
        "PASS screen negative controls: truncated handoff and redraw over prompt are rejected"
    );
    Ok(())
}

fn verify(bytes: &[u8]) -> Result<()> {
    let mut terminal = vt100::Parser::new(16, 88, 0);
    let names = ["github.com/demo/alpha", "github.com/demo/bravo"];
    let mut positions = [BTreeSet::new(), BTreeSet::new()];
    let mut row_numbers = [BTreeSet::new(), BTreeSet::new()];
    let mut last = [None, None];
    let mut directions = [0_i32; 2];
    let mut reversals = [0; 2];
    let mut phase = 0;
    for byte in bytes {
        terminal.process(&[*byte]);
        let contents = terminal.screen().contents();
        let next = match phase {
            0 => contents.contains("alpha authentication"),
            1 => contents.contains("Synthetic token:"),
            2 => contents.contains("Authenticated alpha."),
            3 => contents.contains("bravo authentication"),
            4 => contents.matches("Synthetic token:").count() == 2,
            5 => contents.contains("Authenticated bravo."),
            6 => contents.contains("Saving workspace"),
            7 => contents.contains("Done. Two prompts served in order; terminal display restored."),
            _ => false,
        };
        if next {
            phase += 1;
        }
        ensure!(
            !contents.contains("bravo authentication") || phase >= 4,
            "bravo prompt appeared before alpha completed"
        );
        ensure!(
            !contents.contains("Saving workspace") || phase >= 7,
            "work resumed before native child completion"
        );
        if phase > 0 {
            ensure!(
                !names.iter().any(|name| contents.contains(name)),
                "repository row survived handoff or redrew over native prompt"
            );
            if phase < 6 || phase == 8 {
                ensure!(
                    !contents.contains('█') && !contents.contains('░'),
                    "progress track overlaps native interaction or survives final cleanup"
                );
            }
            continue;
        }
        for (i, name) in names.iter().enumerate() {
            let rows: Vec<_> = contents
                .lines()
                .enumerate()
                .filter(|(_, row)| row.contains(name))
                .collect();
            ensure!(
                rows.len() <= 1,
                "duplicate live row for {name}: {contents:?}"
            );
            if let Some((row, text)) = rows.first() {
                let bar: String = text.chars().take(10).collect();
                if !bar.starts_with('[') || !bar.ends_with(']') {
                    continue;
                }
                if let Some(position) = bar.chars().position(|c| c == '█') {
                    row_numbers[i].insert(*row);
                    positions[i].insert(position);
                    if let Some(previous) = last[i] {
                        let direction = (position as i32 - previous as i32).signum();
                        if direction != 0 {
                            if directions[i] != 0 && direction != directions[i] {
                                reversals[i] += 1;
                            }
                            directions[i] = direction;
                        }
                    }
                    last[i] = Some(position);
                }
            }
        }
    }
    for (i, name) in names.iter().enumerate() {
        ensure!(
            positions[i].len() == 7,
            "{name}: expected seven cursor positions, got {:?}",
            positions[i]
        );
        ensure!(
            row_numbers[i].len() == 1,
            "{name}: row moved or scrolled: {:?}",
            row_numbers[i]
        );
        ensure!(
            reversals[i] >= 4,
            "{name}: expected repeated bouncing, got {} reversals",
            reversals[i]
        );
    }
    ensure!(phase == 8, "incomplete visual lifecycle: phase {phase}/8");
    ensure!(
        !terminal.screen().hide_cursor(),
        "terminal cursor left hidden"
    );
    Ok(())
}
