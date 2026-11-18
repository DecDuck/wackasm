//! A small single-threaded async executor driven by OpenComputers: tasks
//! wait on component calls, signals and timers, and the computer sleeps until
//! one of them is ready. Enable the crate's `async` feature to use it.
//!
//! ```no_run
//! use std::time::Duration;
//! use wackasm_opencomputers::{prelude::*, task};
//!
//! task::run(async {
//!     // A background task: blink a cursor in the corner every half second.
//!     task::spawn(async {
//!         let gpu = Gpu::first().expect("no GPU");
//!         let mut on = false;
//!         loop {
//!             on = !on;
//!             let _ = gpu.set_async(1, 1, if on { "_" } else { " " }, None).await;
//!             task::sleep(Duration::from_millis(500)).await;
//!         }
//!     });
//!
//!     // Meanwhile: two component calls in flight at once, then wait for a key.
//!     let gpu = Gpu::first().expect("no GPU");
//!     let (resolution, depth) = task::join(gpu.get_resolution_async(), gpu.get_depth_async()).await;
//!     println!("{resolution:?} at depth {depth:?}");
//!     let key = task::signal(|s| matches!(s, Signal::KeyDown { .. })).await;
//!     println!("pressed {key:?}");
//! });
//! ```
//!
//! # How it works
//!
//! The `_async` methods queue their component call with the architecture and
//! return a future at once; the call runs on the server while the program
//! keeps going, and the future completes when the result comes back. Calls
//! run in the order they were made. A direct call runs as soon as the program
//! next waits; indirect calls run on the server tick, as many per tick as the
//! server's `asyncCallsPerTick` allows (1 by default, like Lua).
//!
//! [`run`] polls every task that is ready, then waits for whatever comes
//! first: a completed call, a signal, or the next timer. Signals go to the
//! first task waiting for one that matches ([`signal`]); signals nobody is
//! waiting for are kept (up to 256) for later.
//!
//! Everything blocking still works inside tasks: `std::fs`, `println!`,
//! reading stdin, the blocking component methods and [`crate::process`].
//! While one runs, though, no other task does, so prefer the `_async`
//! versions in code that has other tasks to keep going. Don't call
//! [`crate::signal::pull`] inside [`run`]: it would take signals other tasks
//! are waiting for. Use [`signal`].

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, VecDeque},
    future::Future,
    pin::Pin,
    rc::Rc,
    sync::{Arc, Mutex},
    task::{Context, Poll, Wake, Waker},
    time::Duration,
};

use crate::{
    component,
    error::{Error, Result},
    signal::Signal,
    sys,
    value::Value,
};

/// Signals kept for later when no task is waiting for them.
const MAX_BUFFERED_SIGNALS: usize = 256;

type BoxedTask = Pin<Box<dyn Future<Output = ()>>>;

enum Slot {
    Empty,
    /// Taken out while being polled.
    Polling,
    Ready(BoxedTask),
}

struct TaskWaker {
    id: usize,
    ready: Arc<Mutex<VecDeque<usize>>>,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        if let Ok(mut ready) = self.ready.lock() {
            ready.push_back(self.id);
        }
    }
}

enum CallSlot {
    Waiting(Option<Waker>),
    Done(Result<Vec<Value>>),
}

struct SignalWaiter {
    id: u64,
    filter: Box<dyn Fn(&Signal) -> bool>,
    waker: Option<Waker>,
}

#[derive(Default)]
struct Reactor {
    calls: HashMap<i64, CallSlot>,
    /// By (deadline in ms of uptime, id).
    timers: BTreeMap<(u64, u64), Waker>,
    signal_waiters: Vec<SignalWaiter>,
    /// Signals delivered to a waiter, by waiter id.
    delivered: HashMap<u64, Signal>,
    /// Signals nobody was waiting for.
    unclaimed: VecDeque<Signal>,
    next_id: u64,
}

impl Reactor {
    fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn deliver(&mut self, signal: Signal) {
        match self.signal_waiters.iter().position(|w| (w.filter)(&signal)) {
            Some(i) => {
                let waiter = self.signal_waiters.remove(i);
                self.delivered.insert(waiter.id, signal);
                if let Some(waker) = waiter.waker {
                    waker.wake();
                }
            }
            None => {
                if self.unclaimed.len() == MAX_BUFFERED_SIGNALS {
                    self.unclaimed.pop_front();
                }
                self.unclaimed.push_back(signal);
            }
        }
    }

    fn fire_timers(&mut self, now: u64) {
        while let Some(entry) = self.timers.first_entry() {
            if entry.key().0 > now {
                break;
            }
            entry.remove().wake();
        }
    }

    /// Anything that could wake a task.
    fn has_waiters(&self) -> bool {
        !self.calls.is_empty() || !self.timers.is_empty() || !self.signal_waiters.is_empty()
    }
}

struct Runtime {
    tasks: RefCell<Vec<Slot>>,
    ready: Arc<Mutex<VecDeque<usize>>>,
    reactor: RefCell<Reactor>,
}

thread_local! {
    static RUNTIME: RefCell<Option<Rc<Runtime>>> = const { RefCell::new(None) };
}

fn runtime() -> Rc<Runtime> {
    RUNTIME
        .with(|r| r.borrow().clone())
        .expect("wackasm_opencomputers async calls, timers and signals must run inside task::run")
}

fn with_reactor<T>(f: impl FnOnce(&mut Reactor) -> T) -> T {
    let rt = runtime();
    let mut reactor = rt.reactor.borrow_mut();
    f(&mut reactor)
}

fn now_ms() -> u64 {
    unsafe { sys::uptime_ms() }.max(0) as u64
}

impl Runtime {
    fn spawn(&self, task: BoxedTask) {
        let mut tasks = self.tasks.borrow_mut();
        let id = match tasks.iter().position(|s| matches!(s, Slot::Empty)) {
            Some(id) => {
                tasks[id] = Slot::Ready(task);
                id
            }
            None => {
                tasks.push(Slot::Ready(task));
                tasks.len() - 1
            }
        };
        drop(tasks);
        if let Ok(mut ready) = self.ready.lock() {
            ready.push_back(id);
        }
    }

    /// Polls every task that is ready. `true` if any was.
    fn poll_ready(&self) -> bool {
        let mut polled = false;
        loop {
            let next = self.ready.lock().ok().and_then(|mut r| r.pop_front());
            let Some(id) = next else { return polled };
            let task = {
                let mut tasks = self.tasks.borrow_mut();
                match tasks.get_mut(id).map(|s| std::mem::replace(s, Slot::Polling)) {
                    Some(Slot::Ready(task)) => Some(task),
                    Some(other) => {
                        tasks[id] = other; // finished, or a duplicate wake
                        None
                    }
                    None => None,
                }
            };
            let Some(mut task) = task else { continue };
            polled = true;
            let waker = Waker::from(Arc::new(TaskWaker { id, ready: self.ready.clone() }));
            let done = task.as_mut().poll(&mut Context::from_waker(&waker)).is_ready();
            self.tasks.borrow_mut()[id] = if done { Slot::Empty } else { Slot::Ready(task) };
        }
    }

    /// Waits for whatever comes first: a completion, a signal or a timer.
    fn wait(&self) {
        let now = now_ms();
        let (timeout, any) = {
            let mut reactor = self.reactor.borrow_mut();
            reactor.fire_timers(now);
            let next = reactor.timers.keys().next().map(|(deadline, _)| *deadline);
            (next.map_or(-1, |d| d.saturating_sub(now) as i64), reactor.has_waiters())
        };
        if self.ready.lock().is_ok_and(|r| !r.is_empty()) {
            return; // a timer just fired
        }
        if !any {
            panic!("task::run: every task is waiting, but nothing can wake them (a task awaits itself?)");
        }
        let status = unsafe { sys::wait_ms(timeout) };
        if status == sys::STATUS_OK {
            if let Some(Value::Array(mut items)) = Value::decode(&sys::response())
                && !items.is_empty()
                && let Some(name) = items.remove(0).as_str().map(str::to_owned)
            {
                self.reactor.borrow_mut().deliver(Signal::parse(name, items));
            }
        }
        self.drain_completions();
        self.reactor.borrow_mut().fire_timers(now_ms());
    }

    fn drain_completions(&self) {
        while unsafe { sys::completion_next() } == sys::STATUS_OK {
            let Some(Value::Array(items)) = Value::decode(&sys::response()) else { continue };
            let ticket = items.first().and_then(Value::as_i64);
            let status = items.get(1).and_then(Value::as_i64).unwrap_or(-1) as i32;
            let payload = items.get(2).and_then(Value::as_bytes).map(<[u8]>::to_vec).unwrap_or_default();
            let Some(ticket) = ticket else { continue };
            let mut reactor = self.reactor.borrow_mut();
            if let Some(slot) = reactor.calls.get_mut(&ticket) {
                let previous = std::mem::replace(slot, CallSlot::Done(component::decode_reply(status, payload)));
                if let CallSlot::Waiting(Some(waker)) = previous {
                    waker.wake();
                }
            }
        }
    }
}

/// Runs `future` to completion, along with any tasks it [`spawn`]s, and
/// returns its output. Tasks still running when it finishes are dropped.
///
/// # Panics
///
/// If called inside another `run`, or if every task waits on something that
/// can never happen.
pub fn run<F: Future + 'static>(future: F) -> F::Output
where
    F::Output: 'static,
{
    let rt = Rc::new(Runtime {
        tasks: RefCell::new(Vec::new()),
        ready: Arc::new(Mutex::new(VecDeque::new())),
        reactor: RefCell::new(Reactor::default()),
    });
    RUNTIME.with(|r| {
        let mut r = r.borrow_mut();
        assert!(r.is_none(), "task::run can't be nested");
        *r = Some(rt.clone());
    });
    // Uninstall even if a task panics (with panic = "unwind").
    struct Uninstall;
    impl Drop for Uninstall {
        fn drop(&mut self) {
            RUNTIME.with(|r| *r.borrow_mut() = None);
        }
    }
    let _uninstall = Uninstall;

    let output = Rc::new(RefCell::new(None));
    let slot = output.clone();
    rt.spawn(Box::pin(async move {
        *slot.borrow_mut() = Some(future.await);
    }));
    loop {
        rt.poll_ready();
        if let Some(value) = output.borrow_mut().take() {
            // Cancel what is left, so its calls don't run for nobody.
            rt.tasks.borrow_mut().clear();
            return value;
        }
        rt.wait();
    }
}

struct JoinState<T> {
    output: Option<T>,
    waker: Option<Waker>,
}

/// A spawned task's output. Awaiting it waits for the task; dropping it
/// lets the task run on without anyone collecting the output.
pub struct JoinHandle<T> {
    state: Rc<RefCell<JoinState<T>>>,
}

impl<T> Future for JoinHandle<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let mut state = self.state.borrow_mut();
        match state.output.take() {
            Some(output) => Poll::Ready(output),
            None => {
                state.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

/// Starts `future` as a separate task, which runs while this one waits.
pub fn spawn<F: Future + 'static>(future: F) -> JoinHandle<F::Output>
where
    F::Output: 'static,
{
    let state = Rc::new(RefCell::new(JoinState { output: None, waker: None }));
    let task_state = state.clone();
    runtime().spawn(Box::pin(async move {
        let output = future.await;
        let mut state = task_state.borrow_mut();
        state.output = Some(output);
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }));
    JoinHandle { state }
}

/// Waits for both futures, running them at the same time.
pub async fn join<A: Future, B: Future>(a: A, b: B) -> (A::Output, B::Output) {
    let (mut a, mut b) = (Box::pin(a), Box::pin(b));
    let (mut ra, mut rb) = (None, None);
    std::future::poll_fn(|cx| {
        if ra.is_none()
            && let Poll::Ready(v) = a.as_mut().poll(cx)
        {
            ra = Some(v);
        }
        if rb.is_none()
            && let Poll::Ready(v) = b.as_mut().poll(cx)
        {
            rb = Some(v);
        }
        if ra.is_some() && rb.is_some() {
            Poll::Ready((ra.take().unwrap(), rb.take().unwrap()))
        } else {
            Poll::Pending
        }
    })
    .await
}

/// Waits for every future, running them at the same time. The outputs are in
/// the same order as the futures.
pub async fn join_all<F: Future>(futures: impl IntoIterator<Item = F>) -> Vec<F::Output> {
    let mut futures: Vec<Pin<Box<F>>> = futures.into_iter().map(Box::pin).collect();
    let mut outputs: Vec<Option<F::Output>> = futures.iter().map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut pending = false;
        for (future, output) in futures.iter_mut().zip(outputs.iter_mut()) {
            if output.is_none() {
                match future.as_mut().poll(cx) {
                    Poll::Ready(v) => *output = Some(v),
                    Poll::Pending => pending = true,
                }
            }
        }
        if pending { Poll::Pending } else { Poll::Ready(outputs.iter_mut().map(|o| o.take().unwrap()).collect()) }
    })
    .await
}

/// Lets other tasks run before continuing.
pub async fn yield_now() {
    let mut yielded = false;
    std::future::poll_fn(|cx| {
        if yielded {
            Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await
}

/// Waits for `duration` of game time without holding up other tasks.
pub fn sleep(duration: Duration) -> Sleep {
    Sleep { deadline: now_ms().saturating_add(duration.as_millis().min(u64::MAX as u128) as u64), key: None }
}

/// The future [`sleep`] returns.
pub struct Sleep {
    deadline: u64,
    key: Option<(u64, u64)>,
}

impl Future for Sleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if now_ms() >= self.deadline {
            if let Some(key) = self.key.take() {
                with_reactor(|r| r.timers.remove(&key));
            }
            return Poll::Ready(());
        }
        let deadline = self.deadline;
        let key = *self.key.get_or_insert_with(|| (deadline, with_reactor(Reactor::next_id)));
        with_reactor(|r| r.timers.insert(key, cx.waker().clone()));
        Poll::Pending
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            let _ = RUNTIME.with(|rt| rt.borrow().as_ref().map(|rt| rt.reactor.borrow_mut().timers.remove(&key)));
        }
    }
}

/// `future`'s output, or `Err(Error::Failed("timed out"))` if `duration`
/// passes first (`future` is then dropped, cancelling its calls).
pub async fn timeout<F: Future>(duration: Duration, future: F) -> Result<F::Output> {
    let mut future = Box::pin(future);
    let mut timer = Box::pin(sleep(duration));
    std::future::poll_fn(|cx| {
        if let Poll::Ready(v) = future.as_mut().poll(cx) {
            return Poll::Ready(Ok(v));
        }
        match timer.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(Error::Failed("timed out".into()))),
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}

/// The next signal that `filter` accepts. Signals that arrived earlier and
/// that no task was waiting for count too, oldest first.
pub fn signal(filter: impl Fn(&Signal) -> bool + 'static) -> NextSignal {
    NextSignal { filter: Some(Box::new(filter)), id: None }
}

/// The next signal of any kind.
pub fn next_signal() -> NextSignal {
    signal(|_| true)
}

/// The future [`signal`] returns.
pub struct NextSignal {
    filter: Option<Box<dyn Fn(&Signal) -> bool>>,
    id: Option<u64>,
}

impl Future for NextSignal {
    type Output = Signal;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Signal> {
        let this = &mut *self;
        with_reactor(|r| {
            if let Some(id) = this.id {
                if let Some(signal) = r.delivered.remove(&id) {
                    this.id = None;
                    return Poll::Ready(signal);
                }
                if let Some(waiter) = r.signal_waiters.iter_mut().find(|w| w.id == id) {
                    waiter.waker = Some(cx.waker().clone());
                }
                return Poll::Pending;
            }
            let filter = this.filter.take().expect("NextSignal polled after completion");
            if let Some(i) = r.unclaimed.iter().position(|s| filter(s)) {
                return Poll::Ready(r.unclaimed.remove(i).unwrap());
            }
            let id = r.next_id();
            r.signal_waiters.push(SignalWaiter { id, filter, waker: Some(cx.waker().clone()) });
            this.id = Some(id);
            Poll::Pending
        })
    }
}

impl Drop for NextSignal {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else { return };
        let _ = RUNTIME.with(|rt| {
            rt.borrow().as_ref().map(|rt| {
                let mut r = rt.reactor.borrow_mut();
                r.signal_waiters.retain(|w| w.id != id);
                // Delivered but never collected: give it to someone else.
                if let Some(signal) = r.delivered.remove(&id) {
                    r.deliver(signal);
                }
            })
        });
    }
}

/// What an async call is made on.
pub(crate) enum Target {
    Component(String),
    Value(u32),
}

/// A component call in flight; see [`component::invoke_async`].
///
/// The call is queued when the future is first polled. Dropping the future
/// before it completes cancels the call.
#[must_use = "the call is only made when the future is awaited"]
pub struct Call {
    state: CallState,
}

enum CallState {
    Start { target: Target, method: String, args: Vec<Value> },
    Queued(i64),
    Done,
}

impl Call {
    pub(crate) fn new(target: Target, method: &str, args: Vec<Value>) -> Call {
        Call { state: CallState::Start { target, method: method.to_owned(), args } }
    }
}

impl Future for Call {
    type Output = Result<Vec<Value>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match std::mem::replace(&mut self.state, CallState::Done) {
            CallState::Start { target, method, args } => {
                let rt = runtime(); // panics outside `run`, before queuing anything
                let args = Value::Array(args).encode();
                let ticket = unsafe {
                    match &target {
                        Target::Component(address) => sys::invoke_async(
                            address.as_ptr(),
                            address.len(),
                            method.as_ptr(),
                            method.len(),
                            args.as_ptr(),
                            args.len(),
                        ),
                        Target::Value(id) => {
                            sys::invoke_value_async(*id, method.as_ptr(), method.len(), args.as_ptr(), args.len())
                        }
                    }
                };
                if ticket < 0 {
                    let message = String::from_utf8_lossy(&sys::response()).into_owned();
                    return Poll::Ready(Err(Error::Failed(message)));
                }
                rt.reactor.borrow_mut().calls.insert(ticket, CallSlot::Waiting(Some(cx.waker().clone())));
                self.state = CallState::Queued(ticket);
                Poll::Pending
            }
            CallState::Queued(ticket) => with_reactor(|r| match r.calls.remove(&ticket) {
                Some(CallSlot::Done(result)) => Poll::Ready(result),
                Some(CallSlot::Waiting(_)) | None => {
                    r.calls.insert(ticket, CallSlot::Waiting(Some(cx.waker().clone())));
                    self.state = CallState::Queued(ticket);
                    Poll::Pending
                }
            }),
            CallState::Done => panic!("component call polled after completion"),
        }
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        if let CallState::Queued(ticket) = self.state {
            let _ = RUNTIME.with(|rt| rt.borrow().as_ref().map(|rt| rt.reactor.borrow_mut().calls.remove(&ticket)));
            unsafe { sys::cancel(ticket) };
        }
    }
}
