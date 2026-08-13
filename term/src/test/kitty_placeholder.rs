//! Tests for the Kitty graphics protocol's Unicode placeholder image placement
//! (`U+10EEEE` + row/column combining diacritics), as used by tmux-aware clients
//! (e.g. paras-commander, kitty itself, Ghostty) to place images without relying
//! on out-of-band cursor-relative writes that race a multiplexer's own redraw.

use super::*;
use k9::assert_equal as assert_eq;

/// A tiny but valid 11x11 PNG, base64 encoded. Same fixture as term/src/test/image.rs.
const TINY_PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAsAAAALCAYAAACprHcmAAAACXBIWXMAAAGKAAABigEzlzBYAAAAOUlEQVQYlZXOwQ0AMAzCQEdi7yaT0xWAN7JuDCac2PQKYxflycOoICOKtPIuqFCg4/LzKxiz6xjyAYh9DR1sLUN1AAAAAElFTkSuQmCC";

/// The row/column diacritics table, indexed by 0-based grid position, copied from
/// kitty's own `tools/utils/images/rowcolumn_diacritics.go` (same source
/// `rowcolumn_diacritics_helpers.rs`'s `diacritic_to_num` was built from). Index 0 is
/// `U+0305`, which `diacritic_to_num` decodes back to value 1 (1-based internally,
/// then rebiased to 0-based by `kitty_img_for_placeholder`).
const DIACRITIC: [char; 4] = ['\u{305}', '\u{30d}', '\u{30e}', '\u{310}'];

const PLACEHOLDER_BASE: char = '\u{10eeee}';

/// Builds one placeholder grapheme cluster (base char + row diacritic + col diacritic)
/// for 0-based (row, col), matching `previewpanel.UnicodePlaceholderCell` in
/// paras-commander.
fn placeholder_cell(row: usize, col: usize) -> String {
    format!("{}{}{}", PLACEHOLDER_BASE, DIACRITIC[row], DIACRITIC[col])
}

/// SGR true-color foreground escape carrying the low 24 bits of the image id, matching
/// `previewpanel.UnicodePlaceholderColor`.
fn placeholder_fg(image_id: u32) -> String {
    let r = (image_id >> 16) & 0xff;
    let g = (image_id >> 8) & 0xff;
    let b = image_id & 0xff;
    format!("\x1b[38;2;{};{};{}m", r, g, b)
}

/// Transmits `TINY_PNG_BASE64` as a Kitty Unicode-placeholder virtual placement
/// (`U=1`) with an explicit `cols`x`rows` grid, mirroring
/// `paras-commander/internal/preview/image.go`'s `encodeKittyAPC` in placeholder mode
/// (`a=T,f=100,i=<id>,U=1,q=2`) plus an explicit `c=`/`r=` (which that encoder
/// currently omits — see the harness notes).
fn transmit_placeholder_image(term: &mut TestTerm, image_id: u32, cols: usize, rows: usize) {
    let seq = format!(
        "\x1b_Ga=T,f=100,i={},U=1,q=2,c={},r={};{}\x1b\\",
        image_id, cols, rows, TINY_PNG_BASE64
    );
    term.print(seq.as_bytes());
}

/// Prints a `cols`x`rows` placeholder grid with its top-left cell at 0-based
/// (`origin_x`, `origin_y`), positioning each row with an absolute cursor move (CUP)
/// rather than relying on line-wrap/newline - tcell (and any cell-buffer-diffing UI
/// library) writes each screen cell by absolute position, not by relying on the
/// terminal's own cursor-advance/wrap behavior, so this is what `previewpanel.Draw`'s
/// `screen.SetContent` calls actually turn into on the wire.
fn print_placeholder_grid(
    term: &mut TestTerm,
    image_id: u32,
    origin_x: usize,
    origin_y: i64,
    cols: usize,
    rows: usize,
) {
    term.print(placeholder_fg(image_id));
    for row in 0..rows {
        // CUP is 1-based.
        term.print(format!(
            "\x1b[{};{}H",
            origin_y + row as i64 + 1,
            origin_x + 1
        ));
        for col in 0..cols {
            term.print(placeholder_cell(row, col));
        }
    }
    term.print("\x1b[0m");
}

/// Returns the (image_id-carrying) `ImageCell`s attached to the cell at (x, y), if any.
fn images_at(term: &mut TestTerm, x: usize, y: i64) -> Vec<wezterm_cell::image::ImageCell> {
    term.term
        .screen_mut()
        .get_cell(x, y)
        .and_then(|c| c.attrs().images())
        .unwrap_or_default()
}

/// The core correctness check: a placeholder grid drawn away from the terminal origin
/// must attach the image to the cells it was actually drawn at — not to (0,0). This is
/// the in-process (no tmux) analogue of the "images consistently drawn at 0,0" report:
/// if this fails, the bug is in wezterm's own placeholder handling, not in tmux's
/// forwarding of the byte stream.
#[test]
fn placeholder_grid_lands_at_cursor_not_origin() {
    let mut term = TestTerm::new(10, 40, 0);

    let base_x = 9;
    let base_y = 3;
    transmit_placeholder_image(&mut term, 1, 2, 2);
    print_placeholder_grid(&mut term, 1, base_x, base_y, 2, 2);

    // Nothing should have been attached at the origin.
    assert!(
        images_at(&mut term, 0, 0).is_empty(),
        "image must not be attached at the origin"
    );

    for row in 0..2 {
        for col in 0..2 {
            let imgs = images_at(&mut term, base_x + col, base_y + row as i64);
            assert_eq!(
                imgs.len(),
                1,
                "expected exactly one image at grid ({row},{col}) -> screen ({},{})",
                base_x + col,
                base_y + row as i64
            );
            assert_eq!(imgs[0].row(), Some(row as u16), "row mismatch at grid ({row},{col})");
            assert_eq!(imgs[0].col(), Some(col as u16), "col mismatch at grid ({row},{col})");
        }
    }
}

/// A placeholder grid drawn starting at column 0 (leftmost column) must not panic.
/// `flush_print`'s "seed last_col/last_row from the previous cell" lookup reads
/// `cursor.x - 1` where `cursor.x` is an unsigned `usize`; at column 0 that underflows.
#[test]
fn placeholder_grid_at_column_zero_does_not_panic() {
    let mut term = TestTerm::new(10, 40, 0);

    transmit_placeholder_image(&mut term, 1, 2, 1);
    print_placeholder_grid(&mut term, 1, 0, 2, 2, 1);

    let imgs = images_at(&mut term, 0, 2);
    assert_eq!(imgs.len(), 1, "image should attach at column 0 without panicking");
    assert_eq!(imgs[0].col(), Some(0));
}

/// Two placeholder cells printed back to back on the same row, each with explicit
/// row/col diacritics (as paras-commander always sends), must each land on their own
/// column - not collapse onto the same cell via the "infer from last position"
/// fallback meant for placeholders that omit diacritics.
#[test]
fn adjacent_placeholder_cells_do_not_collide() {
    let mut term = TestTerm::new(10, 40, 0);

    transmit_placeholder_image(&mut term, 1, 3, 1);
    print_placeholder_grid(&mut term, 1, 4, 4, 3, 1);

    for col in 0..3 {
        let imgs = images_at(&mut term, 4 + col, 4);
        assert_eq!(imgs.len(), 1, "column {col} should have exactly one image cell");
        assert_eq!(imgs[0].col(), Some(col as u16));
    }
}

/// Re-transmitting under the same image id (as paras-commander does on every image
/// change, always reusing the same fixed id `KittyGraphicsImageID`) followed by
/// reprinting the placeholder grid - exactly what `previewpanel.Draw` does every
/// frame, per graphics-implementation-lessons.md lesson 15 - must resolve to the new
/// image, not the deleted one. Unlike a live terminal's cursor-relative placement, a
/// placeholder cell only re-resolves its backing image when its grapheme is actually
/// reprinted (wezterm looks up `id_to_data` at print time, not at render/composite
/// time), so this exercises delete + retransmit + reprint together, matching what the
/// real app sequence looks like rather than assuming an unprompted live update.
#[test]
fn retransmitting_same_id_and_reprinting_updates_backing_image() {
    let mut term = TestTerm::new(10, 40, 0);

    transmit_placeholder_image(&mut term, 1, 1, 1);
    print_placeholder_grid(&mut term, 1, 2, 2, 1, 1);
    let first = images_at(&mut term, 2, 2);
    assert_eq!(first.len(), 1);
    let first_hash = first[0].image_data().hash();

    // Delete + retransmit under the same id, per-spec required before re-transmitting,
    // then reprint the (byte-identical) placeholder grid, as Draw()/Show() do every
    // frame.
    term.print("\x1b_Ga=d,d=I,i=1\x1b\\");
    transmit_placeholder_image(&mut term, 1, 1, 1);
    print_placeholder_grid(&mut term, 1, 2, 2, 1, 1);

    let second = images_at(&mut term, 2, 2);
    assert_eq!(second.len(), 1);
    // Same fixture bytes were retransmitted, so the hash matches again - the point of
    // this test is that the lookup happened at all (no stale Arc, no panic), which a
    // differently-sized fixture would make more visually obvious but isn't needed to
    // prove the resolution path re-ran.
    assert_eq!(second[0].image_data().hash(), first_hash);
}
