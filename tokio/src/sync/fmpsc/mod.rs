//! Alternative (faster) mpsc queue implementation with different API.

mod bounded;
mod unbounded;

use std::{
    fmt,
    mem::{self, ManuallyDrop},
    process,
    ptr::NonNull,
    sync::atomic::{
        AtomicBool,
        Ordering::{AcqRel, Acquire, Release},
    },
    task::{
        Context,
        Poll::{self, Pending, Ready},
    },
};

use loo::mpsc;

use crate::{
    loom::{future::AtomicWaker, sync::atomic::AtomicUsize},
    sync::{mpsc::error::TryRecvError, notify::Notify},
};

pub use crate::sync::mpsc::error;

pub use self::bounded::{channel, Permit, Receiver, Sender};
pub use self::unbounded::{unbounded_channel, UnboundedReceiver, UnboundedSender};

fn channel_inner<T, S: Semaphore>(semaphore: S) -> (Tx<T, S>, Rx<T, S>) {
    let (tx, rx) = mpsc::queue();

    let shared = Box::leak(Box::new(Shared {
        notify_rx_closed: Notify::new(),
        semaphore,
        rx_waker: AtomicWaker::new(),
        rx_closed: AtomicBool::new(false),
    }));

    (
        Tx {
            tx: ManuallyDrop::new(tx),
            shared: shared.into(),
        },
        Rx {
            rx: ManuallyDrop::new(rx),
            shared: shared.into(),
        },
    )
}

/// Channel sender
struct Tx<T, S> {
    tx: ManuallyDrop<mpsc::Producer<T>>,
    shared: NonNull<Shared<S>>,
}

unsafe impl<T: Send, S: Send> Send for Tx<T, S> {}
unsafe impl<T: Send, S: Sync> Sync for Tx<T, S> {}

impl<T, S> Tx<T, S> {
    fn semaphore(&self) -> &S {
        &self.shared().semaphore
    }

    /// Send a message and notify the receiver.
    fn send(&mut self, value: T) {
        unsafe { self.send_unsync(value) };
    }

    unsafe fn send_unsync(&self, value: T) {
        // Push the value
        self.tx.push_back(value);
        // Notify the rx task
        self.shared().rx_waker.wake();
    }

    /// Wake the receive half
    fn wake_rx(&self) {
        self.shared().rx_waker.wake();
    }

    /// Returns `true` if senders belong to the same channel.
    fn same_channel(&self, other: &Self) -> bool {
        self.shared == other.shared
    }

    fn shared(&self) -> &Shared<S> {
        unsafe { self.shared.as_ref() }
    }
}

impl<T, S: Semaphore> Tx<T, S> {
    fn is_closed(&self) -> bool {
        self.shared().semaphore.is_closed()
    }

    async fn closed(&self) {
        // In order to avoid a race condition, we first request a notification,
        // **then** check whether the semaphore is closed. If the semaphore is
        // closed the notification request is dropped.
        let notified = self.shared().notify_rx_closed.notified();

        if self.shared().semaphore.is_closed() {
            return;
        }
        notified.await;
    }
}

impl<T, S> Clone for Tx<T, S> {
    fn clone(&self) -> Tx<T, S> {
        Tx {
            tx: self.tx.clone(),
            shared: self.shared.clone(),
        }
    }
}

impl<T, S> Drop for Tx<T, S> {
    fn drop(&mut self) {
        use loo::DropResult::LastOfAny;

        let res = match unsafe { self.tx.drop_explicit() } {
            Some(res) => res,
            None => return,
        };

        // Close the list, which sends a `Close` message
        self.shared().rx_closed.store(true, Release);
        // Notify the receiver
        self.wake_rx();

        if let LastOfAny = res {
            unsafe { mem::drop(Box::from_raw(self.shared.as_ptr())) };
        }
    }
}

impl<T, S: fmt::Debug> fmt::Debug for Tx<T, S> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("Tx")
            .field("tx", &self.tx)
            .field("shared", &self.shared)
            .finish()
    }
}

/// Channel receiver
struct Rx<T, S: Semaphore> {
    rx: ManuallyDrop<mpsc::Consumer<T>>,
    shared: NonNull<Shared<S>>,
}

unsafe impl<T: Send, S: Semaphore + Send> Send for Rx<T, S> {}
unsafe impl<T: Send, S: Semaphore + Sync> Sync for Rx<T, S> {}

impl<T, S: Semaphore + fmt::Debug> fmt::Debug for Rx<T, S> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("Rx")
            .field("rx", &self.rx)
            .field("shared", &self.shared)
            .finish()
    }
}

impl<T, S: Semaphore> Rx<T, S> {
    fn close(&mut self) {
        if self.shared().rx_closed.swap(true, AcqRel) {
            return;
        }

        /*self.inner.rx_fields.with_mut(|rx_fields_ptr| {
            let rx_fields = unsafe { &mut *rx_fields_ptr };

            if rx_fields.rx_closed {
                return;
            }

            rx_fields.rx_closed = true;
        });*/

        // FIXME: is not called, if the channel was closed by the last sender?
        self.shared().semaphore.close();
        self.shared().notify_rx_closed.notify_waiters();
    }

    /// Receive the next value
    fn recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<T>> {
        // Keep track of task budget
        let coop = ready!(crate::coop::poll_proceed(cx));

        macro_rules! try_recv {
            () => {
                if self.shared().rx_closed.load(Acquire) {
                    assert!(self.shared().semaphore.is_idle());
                    coop.made_progress();
                    return Ready(None);
                }

                match self.rx.pop_front() {
                    Some(value) => {
                        self.shared().semaphore.add_permit();
                        coop.made_progress();
                        return Ready(Some(value));
                    }
                    None => {} // fall through
                }
            };
        }

        try_recv!();

        self.shared().rx_waker.register_by_ref(cx.waker());

        // It is possible that a value was pushed between attempting to read
        // and registering the task, so we have to check the channel a
        // second time here.
        try_recv!();

        if self.shared().rx_closed.load(Acquire) && self.shared().semaphore.is_idle() {
            coop.made_progress();
            Ready(None)
        } else {
            Pending
        }
    }

    /// Try to receive the next value.
    fn try_recv(&mut self) -> Result<T, TryRecvError> {
        if self.shared().rx_closed.load(Acquire) {
            return Err(TryRecvError::Disconnected);
        }

        match self.rx.pop_front() {
            Some(value) => {
                self.shared().semaphore.add_permit();
                return Ok(value);
            }
            None => return Err(TryRecvError::Empty),
        }
    }

    fn shared(&self) -> &Shared<S> {
        unsafe { self.shared.as_ref() }
    }
}

impl<T, S: Semaphore> Drop for Rx<T, S> {
    fn drop(&mut self) {
        use loo::DropResult::LastOfAny;

        self.close();
        while let Some(_) = self.rx.pop_front() {
            self.shared().semaphore.add_permit();
        }

        unsafe {
            if let Some(LastOfAny) = self.rx.drop_explicit() {
                mem::drop(Box::from_raw(self.shared.as_ptr()));
            }
        }
    }
}

trait Semaphore {
    fn is_idle(&self) -> bool;
    fn add_permit(&self);
    fn close(&self);
    fn is_closed(&self) -> bool;
}

impl Semaphore for (crate::sync::batch_semaphore::Semaphore, usize) {
    fn add_permit(&self) {
        self.0.release(1)
    }

    fn is_idle(&self) -> bool {
        self.0.available_permits() == self.1
    }

    fn close(&self) {
        self.0.close();
    }

    fn is_closed(&self) -> bool {
        self.0.is_closed()
    }
}

impl Semaphore for AtomicUsize {
    fn add_permit(&self) {
        let prev = self.fetch_sub(2, Release);
        if prev >> 1 == 0 {
            // Something went wrong
            process::abort();
        }
    }

    fn is_idle(&self) -> bool {
        self.load(Acquire) >> 1 == 0
    }

    fn close(&self) {
        self.fetch_or(1, Release);
    }

    fn is_closed(&self) -> bool {
        self.load(Acquire) & 1 == 1
    }
}

struct Shared<S> {
    /// Notifies all tasks listening for the receiver being dropped
    notify_rx_closed: Notify,
    /// Coordinates access to channel's capacity.
    semaphore: S,
    /// Receiver waker. Notified when a value is pushed into the channel.
    rx_waker: AtomicWaker,
    /// Flag indicating if ..
    rx_closed: AtomicBool,
}

impl<S> fmt::Debug for Shared<S>
where
    S: fmt::Debug,
{
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("Chan")
            .field("semaphore", &self.semaphore)
            .field("rx_waker", &self.rx_waker)
            .field("rx_closed", &self.rx_closed)
            .finish()
    }
}
