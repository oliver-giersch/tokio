use std::{
    marker::PhantomData,
    sync::atomic::Ordering,
    task::{Context, Poll},
};

use super::error::{SendError, TryRecvError};

/// No capacity
type Semaphore = crate::loom::sync::atomic::AtomicUsize;

/// TODO...
pub fn unbounded_channel<T>() -> (UnboundedSender<T>, UnboundedReceiver<T>) {
    let (tx, rx) = super::channel_inner(crate::loom::sync::atomic::AtomicUsize::new(0));
    (UnboundedSender { tx }, UnboundedReceiver { rx })
}

/// TODO...
#[derive(Debug)]
pub struct UnboundedSender<T> {
    tx: super::Tx<T, Semaphore>,
}

impl<T> Clone for UnboundedSender<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
        }
    }
}

impl<T> UnboundedSender<T> {
    /// TODO...
    pub fn send(&mut self, value: T) -> Result<(), SendError<T>> {
        unsafe { send_unsync(&self.tx, value) }
    }

    /// TODO...
    pub async fn closed(&self) {
        self.tx.closed().await
    }

    /// TODO...
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    /// TODO...
    pub fn same_channel(&self, other: &Self) -> bool {
        self.tx.same_channel(&other.tx)
    }
}

unsafe fn send_unsync<T>(tx: &super::Tx<T, Semaphore>, value: T) -> Result<(), SendError<T>> {
    if !increase_msg_count(tx.semaphore()) {
        return Err(SendError(value));
    }

    tx.send_unsync(value);
    Ok(())
}

fn increase_msg_count(semaphore: &Semaphore) -> bool {
    const CLOSED_BIT: usize = 0x1;

    let mut curr = semaphore.load(Ordering::Acquire);
    loop {
        if curr & CLOSED_BIT == CLOSED_BIT {
            return false;
        }

        if curr == usize::MAX ^ 1 {
            std::process::abort();
        }

        match semaphore.compare_exchange(curr, curr + 2, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(actual) => {
                curr = actual;
            }
        }
    }
}

/// TODO...
pub struct LocalUnboundedSender<'a, T> {
    tx: &'a super::Tx<T, Semaphore>,
    _marker: PhantomData<()>,
}

// unsafe impl<T> !Send for LocalUnboundedSender<'_, T> {}
unsafe impl<T> Sync for LocalUnboundedSender<'_, T> {}

impl<T> Clone for LocalUnboundedSender<'_, T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx,
            _marker: PhantomData,
        }
    }
}

impl<T> Copy for LocalUnboundedSender<'_, T> {}

impl<T> LocalUnboundedSender<'_, T> {
    pub fn send(&mut self, value: T) -> Result<(), SendError<T>> {
        unsafe { send_unsync(self.tx, value) }
    }
}

/// TODO...
#[derive(Debug)]
pub struct UnboundedReceiver<T> {
    rx: super::Rx<T, Semaphore>,
}

impl<T> UnboundedReceiver<T> {
    /// TODO...
    pub async fn recv(&mut self) -> Option<T> {
        crate::future::poll_fn(|cx| self.poll_recv(cx)).await
    }

    /// TODO...
    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
        self.rx.try_recv()
    }

    /// TODO...
    #[cfg(feature = "sync")]
    pub fn blocking_recv(&mut self) -> Option<T> {
        crate::future::block_on(self.recv())
    }

    /// TODO...
    pub fn close(&mut self) {
        self.rx.close();
    }

    /// TODO...
    pub fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<T>> {
        self.rx.recv(cx)
    }
}
