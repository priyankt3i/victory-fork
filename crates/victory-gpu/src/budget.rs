//! How much of the PC's memory the layer's caches may keep
//! (`VICTORY_MEMORY_BUDGET`).
//!
//! The console's 16 GB are shared by its CPU and graphics processor, and the
//! title has about 13.5 GB of them. Here the title's memory is in the PC's
//! memory, the card gets copies of what it reads, and what does not fit on
//! the card stays in the PC's memory too. On top of that the layer keeps
//! things it made so that it need not make them again: host lists that have
//! run (each with two command allocators, which keep the most memory their
//! recordings ever took), spare scratch memory for lists, blocks of texture
//! staging buffers. Measured on a 64 GB PC, all of it came to 40-46 GB of
//! private memory (lists' allocators about 9 GB, windows of title memory 7,
//! uploads 4). Sized for 64 GB, those caches page a 32 GB PC out.
//!
//! So the caches are sized by the PC's memory ([`Tier`]), and whatever is
//! kept is cut to its least while the PC is short of free memory
//! ([`pressed`]): paging costs far more frames than making a list or a
//! buffer again. What is used in a frame is never taken away: only what was
//! not needed for a while ([`Limits::list_window_ms`]).
//!
//! `VICTORY_MEMORY_BUDGET`: `auto` (the default: by the PC's memory),
//! `large` (as before, for 48 GB and more), `32` (for 24 to 48 GB), `16`
//! (less), or a number of GB, which picks the tier for that much.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;

/// The kind of PC the caches are sized for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tier {
    /// Under 24 GB.
    Small,
    /// 24 to 48 GB: a 32 GB PC.
    Mid,
    /// 48 GB and more: the caches as they were measured on 64 GB.
    Large,
}

/// What the caches may keep, by [`Tier`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Limits {
    pub(crate) tier: Tier,
    /// Host lists that have run kept for the next recording, at most, and
    /// the upload memory they may keep between them (`crate::list`).
    pub(crate) idle_lists: usize,
    pub(crate) idle_list_bytes: u64,
    /// Whether idle lists that no recording needed for
    /// [`Limits::list_window_ms`] are let go, all but
    /// [`Limits::list_margin`] of them.
    pub(crate) trim_lists: bool,
    pub(crate) list_window_ms: u64,
    pub(crate) list_margin: usize,
    /// Scratch memory no list holds, kept at most while memory given back
    /// is kept (`crate::host::keep_freed`), and otherwise.
    pub(crate) scratch_kept: u64,
    pub(crate) scratch_most: u64,
    /// Empty 64 MiB blocks of texture staging buffers kept (`crate::pool`).
    pub(crate) staging_blocks: usize,
    /// Read-back buffers of texture copy-backs kept for the next one
    /// (`crate::texback`), in bytes.
    pub(crate) readback_kept: u64,
    /// Free physical memory under which everything kept is cut to its
    /// least ([`pressed`]).
    pub(crate) floor: u64,
}

impl Limits {
    pub(crate) const fn of(tier: Tier) -> Limits {
        match tier {
            Tier::Large => Limits {
                tier,
                idle_lists: 512,
                idle_list_bytes: 256 * MIB,
                trim_lists: false,
                list_window_ms: 10_000,
                list_margin: 64,
                scratch_kept: GIB,
                scratch_most: 128 * MIB,
                staging_blocks: 8,
                readback_kept: 128 * MIB,
                floor: 2 * GIB,
            },
            Tier::Mid => Limits {
                tier,
                idle_lists: 512,
                idle_list_bytes: 128 * MIB,
                trim_lists: true,
                list_window_ms: 10_000,
                list_margin: 32,
                scratch_kept: 384 * MIB,
                scratch_most: 128 * MIB,
                staging_blocks: 4,
                readback_kept: 64 * MIB,
                floor: 3 * GIB,
            },
            Tier::Small => Limits {
                tier,
                idle_lists: 256,
                idle_list_bytes: 64 * MIB,
                trim_lists: true,
                list_window_ms: 5_000,
                list_margin: 16,
                scratch_kept: 128 * MIB,
                scratch_most: 64 * MIB,
                staging_blocks: 2,
                readback_kept: 32 * MIB,
                floor: 2 * GIB,
            },
        }
    }

    /// Scratch memory kept at most now: the least while the PC is short
    /// of memory.
    pub(crate) fn scratch(&self, keep_freed: bool, pressed: bool) -> u64 {
        if keep_freed && !pressed { self.scratch_kept } else { self.scratch_most }
    }

    /// Read-back buffers kept now: a sixteenth while the PC is short of
    /// memory.
    pub(crate) fn readback(&self, pressed: bool) -> u64 {
        if pressed { self.readback_kept / 16 } else { self.readback_kept }
    }

    /// Empty staging blocks kept now.
    pub(crate) fn staging(&self, pressed: bool) -> usize {
        if pressed { self.staging_blocks.min(1) } else { self.staging_blocks }
    }
}

/// The tier for a PC with `total` bytes of physical memory. (Windows gives
/// a little less than what is fitted: 31.8 GB of 32.)
pub(crate) fn tier_for(total: u64) -> Tier {
    if total >= 46 * GIB {
        Tier::Large
    } else if total >= 22 * GIB {
        Tier::Mid
    } else {
        Tier::Small
    }
}

/// The tier `VICTORY_MEMORY_BUDGET` asks for (`None`: by the PC's memory).
pub(crate) fn tier_asked(value: &str) -> Option<Tier> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "auto" => None,
        "large" | "64" | "off" => Some(Tier::Large),
        "mid" | "32" => Some(Tier::Mid),
        "small" | "16" => Some(Tier::Small),
        other => other.trim_end_matches("gb").trim().parse::<u64>().ok().map(|gb| tier_for(gb * GIB)),
    }
}

/// The PC's physical memory in bytes.
#[cfg(windows)]
fn total_physical_ram() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: plain data the call fills in.
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: a sized structure.
    let ok = unsafe { GlobalMemoryStatusEx(&mut m) } != 0;
    ok.then_some(m.ullTotalPhys)
}

#[cfg(not(windows))]
fn total_physical_ram() -> Option<u64> {
    None
}

/// The limits of this run, chosen once and said in the log.
pub(crate) fn limits() -> &'static Limits {
    static LIMITS: OnceLock<Limits> = OnceLock::new();
    LIMITS.get_or_init(|| {
        let total = total_physical_ram();
        let asked = std::env::var("VICTORY_MEMORY_BUDGET").ok();
        let tier = asked
            .as_deref()
            .and_then(tier_asked)
            .unwrap_or_else(|| total.map_or(Tier::Large, tier_for));
        let l = Limits::of(tier);
        tracing::info!(
            tier = ?l.tier,
            pc_gb = total.map(|t| t as f64 / GIB as f64).unwrap_or(0.0),
            asked = asked.as_deref().unwrap_or("auto"),
            "memory budget (VICTORY_MEMORY_BUDGET): idle host lists at most {}{}, spare scratch {} MB ({} MB short of memory), \
             staging blocks {}, everything kept cut to its least under {} GB free",
            l.idle_lists,
            if l.trim_lists { format!(", those unused for {} s let go but {}", l.list_window_ms / 1000, l.list_margin) } else { String::new() },
            l.scratch_kept / MIB,
            l.scratch_most / MIB,
            l.staging_blocks,
            l.floor / GIB,
        );
        l
    })
}

/// Whether the PC is short of free physical memory now (under
/// [`Limits::floor`]), asked of Windows at most twice a second. A change is
/// said in the log.
pub(crate) fn pressed() -> bool {
    static AT: AtomicU64 = AtomicU64::new(0);
    static PRESSED: AtomicBool = AtomicBool::new(false);
    let now = now_ms().max(1);
    let at = AT.load(Ordering::Relaxed);
    if (at == 0 || now >= at + 500) && AT.compare_exchange(at, now, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
        #[cfg(windows)]
        let free = crate::host::available_physical_ram();
        #[cfg(not(windows))]
        let free: Option<u64> = None;
        let floor = limits().floor;
        let short = free.is_some_and(|f| f < floor);
        if PRESSED.swap(short, Ordering::Relaxed) != short {
            let free_gb = free.map_or(0.0, |f| f as f64 / GIB as f64);
            if short {
                tracing::warn!(free_gb, "the PC is short of memory: what the layer keeps is cut to its least until {} GB are free", floor / GIB);
            } else {
                tracing::info!(free_gb, "the PC has memory free again: the layer keeps what its budget allows");
            }
        }
    }
    PRESSED.load(Ordering::Relaxed)
}

/// Milliseconds since the first ask.
fn now_ms() -> u64 {
    static EPOCH: OnceLock<std::time::Instant> = OnceLock::new();
    EPOCH.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

/// How many of `len` idle host lists to let go, the fewest there were since
/// the last trim being `low` (`usize::MAX`: none was taken meanwhile): those
/// that were never needed, but `margin` of them (`crate::list`).
pub(crate) fn lists_to_let_go(low: usize, len: usize, margin: usize) -> usize {
    low.min(len).saturating_sub(margin)
}

/// Host lists let go because no recording needed them, since the start
/// (`crate::list`).
pub(crate) static LISTS_TRIMMED: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pc_is_sized_by_what_windows_says_it_has() {
        // What Windows reports for 16, 32 and 64 GB fitted.
        assert_eq!(tier_for(15_800 * MIB), Tier::Small);
        assert_eq!(tier_for(31_800 * MIB), Tier::Mid);
        assert_eq!(tier_for(32 * GIB), Tier::Mid);
        assert_eq!(tier_for(48 * GIB), Tier::Large);
        assert_eq!(tier_for(63_800 * MIB), Tier::Large);
    }

    #[test]
    fn the_switch_names_a_tier_or_a_size() {
        assert_eq!(tier_asked("auto"), None);
        assert_eq!(tier_asked(""), None);
        assert_eq!(tier_asked("large"), Some(Tier::Large));
        assert_eq!(tier_asked("32"), Some(Tier::Mid));
        assert_eq!(tier_asked("16"), Some(Tier::Small));
        assert_eq!(tier_asked("24GB"), Some(Tier::Mid));
        assert_eq!(tier_asked("128"), Some(Tier::Large));
        assert_eq!(tier_asked("nonsense"), None);
    }

    /// The idle lists of `crate::list` as a stack of numbers: taken from
    /// the top, given back on top, trimmed from the bottom.
    struct Model {
        idle: Vec<u32>,
        low: usize,
        made: u32,
        let_go: usize,
    }

    impl Model {
        fn take(&mut self) {
            if self.idle.pop().is_none() {
                self.made += 1;
            }
            self.low = self.low.min(self.idle.len());
        }

        fn give(&mut self, n: usize) {
            self.idle.extend(std::iter::repeat_n(0, n));
        }

        fn trim(&mut self, margin: usize) {
            let n = lists_to_let_go(self.low, self.idle.len(), margin);
            self.idle.drain(..n);
            self.let_go += n;
            self.low = self.idle.len();
        }

        /// A frame that has `n` lists out at once, all given back after it.
        fn frame(&mut self, n: usize) {
            (0..n).for_each(|_| self.take());
            self.give(n);
        }
    }

    #[test]
    fn idle_lists_a_frame_needs_are_never_let_go() {
        let margin = Limits::of(Tier::Mid).list_margin;
        let mut m = Model { idle: Vec::new(), low: usize::MAX, made: 0, let_go: 0 };
        // A loading burst of 400 lists out at once, then play at 80 a frame,
        // a window of 300 frames between trims.
        m.frame(400);
        assert_eq!(m.made, 400);
        // The window with the burst in it lets nothing go: all were needed.
        m.trim(margin);
        assert_eq!(m.idle.len(), 400);
        for window in 0..10 {
            (0..300).for_each(|_| m.frame(80));
            m.trim(margin);
            // What play needs stays, and the margin above it.
            assert_eq!(m.idle.len(), 80 + margin, "window {window}");
        }
        assert_eq!(m.let_go, 400 - 80 - margin);
        // Play never made a list once the burst had made them.
        assert_eq!(m.made, 400);
        // Busier play (120 a frame) makes only the 120 - 80 - margin more.
        (0..300).for_each(|_| m.frame(120));
        assert_eq!(m.made, 400 + (120 - 80 - margin) as u32);
        m.trim(margin);
        // (The pool ran dry in that window: nothing goes, it holds the peak.)
        assert_eq!(m.idle.len(), 120);
        (0..300).for_each(|_| m.frame(120));
        m.trim(margin);
        assert_eq!((m.idle.len(), m.made), (120, 408), "steady at the peak, nothing made");
        // A window in which nothing was taken (a pause): all but the margin go.
        m.low = usize::MAX;
        m.trim(margin);
        assert_eq!(m.idle.len(), margin);
        assert_eq!(lists_to_let_go(usize::MAX, 10, 32), 0);
        assert_eq!(lists_to_let_go(5, 100, 2), 3);
    }

    #[test]
    fn the_large_tier_keeps_what_was_kept_before() {
        let l = Limits::of(Tier::Large);
        assert_eq!((l.idle_lists, l.idle_list_bytes), (512, 256 * MIB));
        assert_eq!((l.scratch_kept, l.scratch_most, l.staging_blocks), (GIB, 128 * MIB, 8));
        assert!(!l.trim_lists);
    }

    #[test]
    fn short_of_memory_everything_is_cut_to_its_least() {
        for tier in [Tier::Small, Tier::Mid, Tier::Large] {
            let l = Limits::of(tier);
            assert_eq!(l.scratch(true, true), l.scratch_most);
            assert_eq!(l.scratch(false, false), l.scratch_most);
            assert_eq!(l.scratch(true, false), l.scratch_kept);
            assert!(l.staging(true) <= 1);
            assert!(l.readback(true) < l.readback(false));
            assert!(l.scratch_most <= l.scratch_kept);
        }
        // Smaller PCs keep less.
        let (s, m, g) = (Limits::of(Tier::Small), Limits::of(Tier::Mid), Limits::of(Tier::Large));
        assert!(s.scratch_kept < m.scratch_kept && m.scratch_kept < g.scratch_kept);
        assert!(s.staging_blocks < m.staging_blocks && m.staging_blocks < g.staging_blocks);
        assert!(s.idle_list_bytes < m.idle_list_bytes && m.idle_list_bytes < g.idle_list_bytes);
    }
}
