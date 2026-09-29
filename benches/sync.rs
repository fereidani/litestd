//! litestd's locks, one-time initialization, threads, thread-locals and
//! clocks against std's. Multithreaded benchmarks report the time of a whole
//! run: its threads start together and share one primitive.

#![allow(clippy::unwrap_used, reason = "a failure ends the benchmark")]
#![allow(
    clippy::missing_const_for_thread_local,
    reason = "clippy misreads std's thread_local! where it uses OS keys"
)]

mod common;

use core::{
    cell::{Cell, RefCell},
    hint::black_box,
    sync::atomic::{AtomicU32, AtomicUsize, Ordering},
    time::Duration,
};

use common::compare;

/// Lock operations per thread in a contended run.
const OPS: usize = 10_000;
/// Round trips in a ping-pong run, and generations in a barrier run.
const ROUNDS: u32 = 1_000;

fn main() {
    mutex();
    rwlock();
    condvar();
    once();
    barrier();
    threads();
    thread_locals();
    park();
    clocks();
    scheduler();
}

/// Keeps a shared primitive on cache lines of its own, so that where each
/// side's primitive lands on the stack cannot decide a benchmark.
#[repr(align(128))]
struct Aligned<T>(T);

impl<T> core::ops::Deref for Aligned<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

/// Runs `work` on `threads` std threads that start together, and waits for
/// them; both sides share this harness.
fn contend(threads: usize, work: &(impl Fn(usize) + Sync)) {
    let ready = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for i in 0..threads {
            let ready = &ready;
            s.spawn(move || {
                ready.fetch_add(1, Ordering::AcqRel);
                while ready.load(Ordering::Acquire) < threads {
                    std::thread::yield_now();
                }
                work(i);
            });
        }
    });
}

/// A reader-writer mix: its name, and whether operation `i` writes.
type Mix = (&'static str, fn(usize) -> bool);

/// Runs `lock` `OPS` times on each of `threads` threads at once.
fn mutex_run(threads: usize, lock: &(impl Fn() + Sync)) {
    contend(threads, &|_| (0..OPS).for_each(|_| lock()));
}

fn mutex() {
    let lite = Aligned(litestd::sync::Mutex::new(0_u64));
    let std = Aligned(std::sync::Mutex::new(0_u64));
    compare(
        "mutex uncontended",
        || *black_box(&lite).lock().unwrap() += 1,
        || *black_box(&std).lock().unwrap() += 1,
    );
    for threads in [2, 4, 8] {
        compare(
            &format!("mutex contended x{threads} ({OPS} ops each)"),
            || mutex_run(threads, &|| *lite.lock().unwrap() += 1),
            || mutex_run(threads, &|| *std.lock().unwrap() += 1),
        );
    }
}

fn rwlock() {
    let lite = Aligned(litestd::sync::RwLock::new(0_u64));
    let std = Aligned(std::sync::RwLock::new(0_u64));
    compare(
        "rwlock read uncontended",
        || {
            black_box(*black_box(&lite).read().unwrap());
        },
        || {
            black_box(*black_box(&std).read().unwrap());
        },
    );
    compare(
        "rwlock write uncontended",
        || *black_box(&lite).write().unwrap() += 1,
        || *black_box(&std).write().unwrap() += 1,
    );
    let mixes: [Mix; 2] = [
        ("read-heavy (1/16 writes)", |i| i % 16 == 0),
        ("write-heavy (3/4 writes)", |i| i % 4 != 0),
    ];
    for (name, is_write) in mixes {
        compare(
            &format!("rwlock {name} x4 ({OPS} ops each)"),
            || {
                contend(4, &|_| {
                    for i in 0..OPS {
                        if is_write(i) {
                            *lite.write().unwrap() += 1;
                        } else {
                            black_box(*lite.read().unwrap());
                        }
                    }
                });
            },
            || {
                contend(4, &|_| {
                    for i in 0..OPS {
                        if is_write(i) {
                            *std.write().unwrap() += 1;
                        } else {
                            black_box(*std.read().unwrap());
                        }
                    }
                });
            },
        );
    }
}

/// Passes a turn back and forth `ROUNDS` times between the caller and a
/// partner thread through a condition variable; `$wait` waits on `$cvar`
/// with `$guard` until `$pred` fails.
macro_rules! condvar_ping_pong {
    ($lib:ident, |$cvar:ident, $guard:ident, $pred:ident| $wait:expr) => {{
        let state = Aligned($lib::sync::Mutex::new(0_u32));
        let $cvar = Aligned($lib::sync::Condvar::new());
        std::thread::scope(|s| {
            s.spawn(|| {
                let mut $guard = state.lock().unwrap();
                for _ in 0..ROUNDS {
                    let $pred = |n: &mut u32| *n % 2 == 0;
                    $guard = $wait;
                    *$guard += 1;
                    $cvar.notify_one();
                }
                drop($guard);
            });
            let mut $guard = state.lock().unwrap();
            for _ in 0..ROUNDS {
                *$guard += 1;
                $cvar.notify_one();
                let $pred = |n: &mut u32| *n % 2 == 1;
                $guard = $wait;
            }
            drop($guard);
        });
    }};
}

/// A timeout that never expires during a benchmark.
const LONG: Duration = Duration::from_secs(60);

fn condvar() {
    let lite = Aligned(litestd::sync::Condvar::new());
    let std = Aligned(std::sync::Condvar::new());
    compare(
        "condvar notify_one no waiter",
        || black_box(&lite).notify_one(),
        || black_box(&std).notify_one(),
    );
    compare(
        "condvar notify_all no waiter",
        || black_box(&lite).notify_all(),
        || black_box(&std).notify_all(),
    );
    compare(
        &format!("condvar ping-pong ({ROUNDS} rounds)"),
        || {
            condvar_ping_pong!(litestd, |cv, guard, pred| cv
                .wait_while(guard, pred)
                .unwrap());
        },
        || {
            condvar_ping_pong!(std, |cv, guard, pred| cv
                .wait_while(guard, pred)
                .unwrap());
        },
    );
    compare(
        &format!("condvar ping-pong timed ({ROUNDS} rounds)"),
        || {
            condvar_ping_pong!(litestd, |cv, guard, pred| cv
                .wait_timeout_while(guard, LONG, pred)
                .unwrap()
                .0);
        },
        || {
            condvar_ping_pong!(std, |cv, guard, pred| cv
                .wait_timeout_while(guard, LONG, pred)
                .unwrap()
                .0);
        },
    );
}

fn once() {
    let (lite, std) = (litestd::sync::Once::new(), std::sync::Once::new());
    lite.call_once(|| {});
    std.call_once(|| {});
    compare(
        "once call_once completed",
        || black_box(&lite).call_once(|| {}),
        || black_box(&std).call_once(|| {}),
    );
    let lite = litestd::sync::OnceLock::from(1_u64);
    let std = std::sync::OnceLock::from(1_u64);
    compare(
        "oncelock get",
        || {
            black_box(black_box(&lite).get());
        },
        || {
            black_box(black_box(&std).get());
        },
    );
    compare(
        "oncelock get_or_init initialized",
        || {
            black_box(black_box(&lite).get_or_init(|| 2));
        },
        || {
            black_box(black_box(&std).get_or_init(|| 2));
        },
    );
    let lite = litestd::sync::LazyLock::new(|| black_box(1_u64));
    let std = std::sync::LazyLock::new(|| black_box(1_u64));
    black_box(*lite + *std);
    compare(
        "lazylock deref initialized",
        || {
            black_box(**black_box(&lite));
        },
        || {
            black_box(**black_box(&std));
        },
    );
}

fn barrier() {
    let lite = Aligned(litestd::sync::Barrier::new(4));
    let std = Aligned(std::sync::Barrier::new(4));
    compare(
        &format!("barrier x4 ({ROUNDS} rounds)"),
        || contend(4, &|_| (0..ROUNDS).for_each(|_| _ = lite.wait())),
        || contend(4, &|_| (0..ROUNDS).for_each(|_| _ = std.wait())),
    );
}

fn threads() {
    compare(
        "thread spawn and join",
        || {
            black_box(litestd::thread::spawn(|| 1).join().unwrap());
        },
        || {
            black_box(std::thread::spawn(|| 1).join().unwrap());
        },
    );
    compare(
        "thread scope with one thread",
        || litestd::thread::scope(|s| _ = s.spawn(|| black_box(1))),
        || std::thread::scope(|s| _ = s.spawn(|| black_box(1))),
    );
    compare(
        "thread current",
        || {
            black_box(litestd::thread::current());
        },
        || {
            black_box(std::thread::current());
        },
    );
    compare(
        "thread current id",
        || {
            black_box(litestd::thread::current().id());
        },
        || {
            black_box(std::thread::current().id());
        },
    );
}

litestd::thread_local! {
    static LITE_CONST: Cell<u64> = const { Cell::new(0) };
    static LITE_LAZY: Cell<u64> = Cell::new(black_box(0));
    static LITE_DROP: RefCell<Vec<u64>> = RefCell::new(vec![1]);
}

std::thread_local! {
    static STD_CONST: Cell<u64> = const { Cell::new(0) };
    static STD_LAZY: Cell<u64> = Cell::new(black_box(0));
    static STD_DROP: RefCell<Vec<u64>> = RefCell::new(vec![1]);
}

/// Defines a function that accesses a thread-local once. Each access runs
/// in its own function, as in real code: inlined into the benchmark loop,
/// std's would have its address hoisted and its increments folded.
macro_rules! access {
    ($name:ident, $key:ident, $body:expr) => {
        #[inline(never)]
        fn $name() {
            $key.with($body);
        }
    };
}

access!(lite_const, LITE_CONST, |c| c.set(c.get() + 1));
access!(std_const, STD_CONST, |c| c.set(c.get() + 1));
access!(lite_lazy, LITE_LAZY, |c| c.set(c.get() + 1));
access!(std_lazy, STD_LAZY, |c| c.set(c.get() + 1));
access!(lite_drop, LITE_DROP, |v| _ = black_box(v.borrow().len()));
access!(std_drop, STD_DROP, |v| _ = black_box(v.borrow().len()));

fn thread_locals() {
    compare("thread_local const init", lite_const, std_const);
    compare("thread_local lazy init", lite_lazy, std_lazy);
    compare("thread_local with destructor", lite_drop, std_drop);
}

/// Passes a turn back and forth `ROUNDS` times between the caller and a
/// scoped thread of the same library, which park until it is theirs.
macro_rules! park_ping_pong {
    ($lib:ident) => {{
        let turn = Aligned(AtomicU32::new(0));
        let caller = $lib::thread::current();
        $lib::thread::scope(|s| {
            let partner = s.spawn(|| {
                for round in 0..ROUNDS {
                    while turn.load(Ordering::Acquire) != 2 * round + 1 {
                        $lib::thread::park();
                    }
                    turn.store(2 * round + 2, Ordering::Release);
                    caller.unpark();
                }
            });
            for round in 0..ROUNDS {
                turn.store(2 * round + 1, Ordering::Release);
                partner.thread().unpark();
                while turn.load(Ordering::Acquire) != 2 * round + 2 {
                    $lib::thread::park();
                }
            }
        });
    }};
}

fn park() {
    compare(
        "park unpark token ready",
        || {
            litestd::thread::current().unpark();
            litestd::thread::park();
        },
        || {
            std::thread::current().unpark();
            std::thread::park();
        },
    );
    compare(
        &format!("park unpark ping-pong ({ROUNDS} rounds)"),
        || park_ping_pong!(litestd),
        || park_ping_pong!(std),
    );
}

fn clocks() {
    compare(
        "instant now",
        || {
            black_box(litestd::time::Instant::now());
        },
        || {
            black_box(std::time::Instant::now());
        },
    );
    let (lite, std) =
        (litestd::time::Instant::now(), std::time::Instant::now());
    compare(
        "instant elapsed",
        || {
            black_box(black_box(lite).elapsed());
        },
        || {
            black_box(black_box(std).elapsed());
        },
    );
    compare(
        "systemtime now",
        || {
            black_box(litestd::time::SystemTime::now());
        },
        || {
            black_box(std::time::SystemTime::now());
        },
    );
}

fn scheduler() {
    compare(
        "yield_now",
        litestd::thread::yield_now,
        std::thread::yield_now,
    );
    compare(
        "available_parallelism",
        || drop(black_box(litestd::thread::available_parallelism())),
        || drop(black_box(std::thread::available_parallelism())),
    );
}
