//! calloop source that reads the Wayland socket and dispatches the session.
//!
//! `prepare_read` runs in `before_sleep`, the socket is read in
//! `before_handle_events`, and dispatch happens in `process_events` before the
//! user callback. The callback can borrow the session again.

use std::cell::RefCell;
use std::io::{self, ErrorKind};
use std::os::fd::OwnedFd;
use std::rc::Rc;

use calloop::{EventIterator, Interest, Mode, Poll, PostAction, Readiness, Token, TokenFactory};
use wayland_client::backend::{ReadEventsGuard, WaylandError};

use crate::error::Error;
use crate::session::Session;

/// Multiplexes the Wayland display connection onto a calloop event loop.
pub struct WaylandPump {
    session: Rc<RefCell<Session>>,
    fd: OwnedFd,
    token: Option<Token>,
    guard: Option<ReadEventsGuard>,
    fatal: Option<io::Error>,
}

impl WaylandPump {
    /// Dup the display file descriptor and share `session` with the daemon.
    pub fn new(session: Rc<RefCell<Session>>) -> Result<Self, Error> {
        let fd = session.borrow().connection_fd()?;
        Ok(Self {
            session,
            fd,
            token: None,
            guard: None,
            fatal: None,
        })
    }
}

impl calloop::EventSource for WaylandPump {
    type Event = ();
    type Metadata = ();
    type Ret = ();
    type Error = io::Error;

    const NEEDS_EXTRA_LIFECYCLE_EVENTS: bool = true;

    fn process_events<F>(
        &mut self,
        _readiness: Readiness,
        _token: Token,
        mut callback: F,
    ) -> Result<PostAction, Self::Error>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        if let Some(err) = self.fatal.take() {
            return Err(err);
        }
        let mut session = match self.session.try_borrow_mut() {
            Ok(session) => session,
            Err(_) => {
                tracing::debug!("wayland session is busy during dispatch");
                callback((), &mut ());
                return Ok(PostAction::Continue);
            }
        };
        let Session { state, queue, .. } = &mut *session;
        loop {
            match queue.dispatch_pending(state) {
                Ok(0) => break,
                Ok(_) => continue,
                Err(err) => {
                    drop(session);
                    return Err(io::Error::other(err.to_string()));
                }
            }
        }
        if let Err(err) = queue.flush() {
            if !is_would_block(&err) {
                drop(session);
                return Err(io::Error::other(err.to_string()));
            }
        }
        drop(session);
        callback((), &mut ());
        if let Some(err) = self.fatal.take() {
            return Err(err);
        }
        Ok(PostAction::Continue)
    }

    #[allow(unsafe_code)]
    fn register(
        &mut self,
        poll: &mut Poll,
        token_factory: &mut TokenFactory,
    ) -> calloop::Result<()> {
        let token = token_factory.token();
        // SAFETY: `self.fd` is an owned dup of the Wayland socket and lives as
        // long as this source. The event loop unregisters it before drop.
        unsafe {
            poll.register(&self.fd, Interest::READ, Mode::Level, token)?;
        }
        self.token = Some(token);
        Ok(())
    }

    fn reregister(
        &mut self,
        poll: &mut Poll,
        token_factory: &mut TokenFactory,
    ) -> calloop::Result<()> {
        let token = token_factory.token();
        poll.reregister(&self.fd, Interest::READ, Mode::Level, token)?;
        self.token = Some(token);
        Ok(())
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        poll.unregister(&self.fd)?;
        self.token = None;
        Ok(())
    }

    fn before_sleep(&mut self) -> calloop::Result<Option<(Readiness, Token)>> {
        self.guard.take();
        let Some(token) = self.token else {
            return Ok(None);
        };
        let session = match self.session.try_borrow() {
            Ok(session) => session,
            Err(_) => {
                tracing::debug!("wayland session is busy; skipping prepare_read");
                return Ok(None);
            }
        };
        if let Err(err) = session.queue.flush() {
            if !is_would_block(&err) {
                return Err(calloop::Error::OtherError(Box::new(io::Error::other(
                    err.to_string(),
                ))));
            }
        }
        let prepared = session.queue.prepare_read();
        drop(session);
        match prepared {
            Some(guard) => {
                self.guard = Some(guard);
                Ok(None)
            }
            None => Ok(Some((
                Readiness {
                    readable: true,
                    writable: false,
                    error: false,
                },
                token,
            ))),
        }
    }

    fn before_handle_events(&mut self, events: EventIterator<'_>) {
        let readable = self.token.is_some_and(|token| {
            events
                .clone()
                .any(|(ready, event_token)| event_token == token && (ready.readable || ready.error))
        });
        let Some(guard) = self.guard.take() else {
            return;
        };
        if !readable {
            drop(guard);
            return;
        }
        match guard.read() {
            Ok(_) => {}
            Err(err) if is_would_block(&err) => {}
            Err(err) => {
                self.fatal = Some(io::Error::other(err.to_string()));
            }
        }
    }
}

fn is_would_block(err: &WaylandError) -> bool {
    matches!(err, WaylandError::Io(inner) if inner.kind() == ErrorKind::WouldBlock)
}
