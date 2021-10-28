use std::{
    fmt,
    marker::PhantomData,
    task::{Context, Poll},
};

use crate::sync::batch_semaphore;

use super::error::{SendError, TryRecvError, TrySendError};

/// Channel semaphore is a tuple of the semaphore implementation and a `usize`
/// representing the channel bound.
type Semaphore = (batch_semaphore::Semaphore, usize);

/// TODO...
pub fn channel<T>(buffer: usize) -> (Sender<T>, Receiver<T>) {
    assert!(buffer > 0, "mpsc bounded channel requires buffer > 0");
    let semaphore = (batch_semaphore::Semaphore::new(buffer), buffer);
    let (tx, rx) = super::channel_inner(semaphore);

    (Sender { tx }, Receiver(rx))
}

/// TODO...
pub struct Sender<T> {
    tx: super::Tx<T, Semaphore>,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
        }
    }
}

impl<T> Sender<T> {
    /// TODO...
    pub async fn send(&mut self, value: T) -> Result<(), SendError<T>> {
        match self.reserve().await {
            Ok(permit) => {
                permit.send(value);
                Ok(())
            }
            Err(_) => Err(SendError(value)),
        }
    }

    /// TODO...
    pub fn try_send(&mut self, message: T) -> Result<(), TrySendError<T>> {
        use batch_semaphore::TryAcquireError;
        match self.tx.semaphore().0.try_acquire(1) {
            Ok(_) => {}
            Err(TryAcquireError::Closed) => return Err(TrySendError::Closed(message)),
            Err(TryAcquireError::NoPermits) => return Err(TrySendError::Full(message)),
        }

        // Send the message
        self.tx.send(message);
        Ok(())
    }

    /// TODO: ...
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    /// TODO...
    pub fn local(&mut self) -> LocalSender<'_, T> {
        LocalSender {
            tx: &self.tx,
            _marker: PhantomData,
        }
    }

    /// TODO...
    pub async fn closed(&self) {
        self.tx.closed().await
    }

    /// TODO...
    pub async fn reserve(&mut self) -> Result<Permit<'_, T>, SendError<()>> {
        self.reserve_inner().await?;
        Ok(Permit { sender: self })
    }

    /// TODO...
    pub fn try_reserve(&mut self) -> Result<Permit<'_, T>, TrySendError<()>> {
        use batch_semaphore::TryAcquireError;
        match self.tx.semaphore().0.try_acquire(1) {
            Ok(_) => {}
            Err(TryAcquireError::Closed) => return Err(TrySendError::Closed(())),
            Err(TryAcquireError::NoPermits) => return Err(TrySendError::Full(())),
        }

        Ok(Permit { sender: self })
    }

    async fn reserve_inner(&mut self) -> Result<(), SendError<()>> {
        match self.tx.semaphore().0.acquire(1).await {
            Ok(_) => Ok(()),
            Err(_) => Err(SendError(())),
        }
    }
}

impl<T> fmt::Debug for Sender<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_tuple("Sender").field(&self.tx).finish()
    }
}

/// TODO...
#[derive(Debug)]
pub struct LocalSender<'a, T> {
    tx: &'a super::Tx<T, Semaphore>,
    _marker: PhantomData<*const ()>,
}

// unsafe impl<T> !Send for LocalSender<'_, T> {}
unsafe impl<T> Sync for LocalSender<'_, T> {}

impl<T> Clone for LocalSender<'_, T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx,
            _marker: PhantomData,
        }
    }
}

impl<T> LocalSender<'_, T> {
    pub async fn send(self, _value: T) -> Result<(), SendError<T>> {
        todo!()
    }
}

/// TODO...
pub struct Receiver<T>(super::Rx<T, Semaphore>);

impl<T> Receiver<T> {
    /// TODO...
    pub async fn recv(&mut self) -> Option<T> {
        crate::future::poll_fn(|cx| self.0.recv(cx)).await
    }

    /// TODO...
    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
        self.0.try_recv()
    }

    /// TODO...
    #[cfg(feature = "sync")]
    pub fn blocking_recv(&mut self) -> Option<T> {
        crate::future::block_on(self.recv())
    }

    /// TODO...
    pub fn close(&mut self) {
        self.0.close();
    }

    /// TODO...
    pub fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<T>> {
        self.0.recv(cx)
    }
}

impl<T> fmt::Debug for Receiver<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_tuple("Receiver").field(&self.0).finish()
    }
}

impl<T> Unpin for Receiver<T> {}

/// TODO...
pub struct Permit<'a, T> {
    sender: &'a mut Sender<T>,
}

impl<T> Permit<'_, T> {
    /// Sends a value using the reserved capacity.
    ///
    /// Capacity for the message has already been reserved. The message is sent
    /// to the receiver and the permit is consumed. The operation will succeed
    /// even if the receiver half has been closed. See [`Receiver::close`] for
    /// more details on performing a clean shutdown.
    ///
    /// [`Receiver::close`]: Receiver::close
    ///
    /// # Examples
    ///
    /// ```
    /// use tokio::sync::mpsc;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let (tx, mut rx) = mpsc::channel(1);
    ///
    ///     // Reserve capacity
    ///     let permit = tx.reserve().await.unwrap();
    ///
    ///     // Trying to send directly on the `tx` will fail due to no
    ///     // available capacity.
    ///     assert!(tx.try_send(123).is_err());
    ///
    ///     // Send a message on the permit
    ///     permit.send(456);
    ///
    ///     // The value sent on the permit is received
    ///     assert_eq!(rx.recv().await.unwrap(), 456);
    /// }
    /// ```
    pub fn send(self, value: T) {
        use std::mem;

        self.sender.tx.send(value);
        // Avoid the drop logic
        mem::forget(self);
    }
}

impl<T> Drop for Permit<'_, T> {
    fn drop(&mut self) {
        use super::Semaphore;

        let semaphore = self.sender.tx.semaphore();
        // Add the permit back to the semaphore
        semaphore.add_permit();
        // If this is the last sender for this channel, wake the receiver so
        // that it can be notified that the channel is closed.
        if semaphore.is_closed() && semaphore.is_idle() {
            self.sender.tx.wake_rx();
        }
    }
}

impl<T> fmt::Debug for Permit<'_, T> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("Permit")
            .field("sender", &self.sender)
            .finish()
    }
}
