//! Wayland connection to a target: registry, binding, dispatch.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use anyhow::{Context, Result};
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

use crate::target::{self, Env};

#[derive(Default)]
pub struct State {
    /// Last ext_idle_notification_v1 event: idled (true) or resumed.
    pub idled: bool,
}

pub struct Wl {
    pub queue: EventQueue<State>,
    pub qh: QueueHandle<State>,
    pub globals: GlobalList,
    pub state: State,
}

fn socket_path(display: &str) -> Result<PathBuf> {
    let p = PathBuf::from(display);
    Ok(if p.is_absolute() {
        p
    } else {
        target::runtime_dir()?.join(p)
    })
}

impl Wl {
    /// Connect to the target's compositor and learn its outputs' names.
    pub fn connect(env: &Env) -> Result<Wl> {
        let path = socket_path(&env.wayland_display)?;
        let stream = UnixStream::connect(&path)
            .with_context(|| format!("connecting to {}", path.display()))?;
        let conn = Connection::from_socket(stream)?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
        let qh = queue.handle();
        let mut state = State::default();
        queue.roundtrip(&mut state)?;
        Ok(Wl {
            queue,
            qh,
            globals,
            state,
        })
    }

    pub fn bind<I>(&self, versions: std::ops::RangeInclusive<u32>) -> Result<I>
    where
        I: Proxy + 'static,
        State: Dispatch<I, ()>,
    {
        self.globals
            .bind(&self.qh, versions, ())
            .with_context(|| format!("the compositor has no {}", I::interface().name))
    }

    /// Send everything queued, in one write when it fits.
    pub fn flush(&mut self) -> Result<()> {
        self.queue.flush()?;
        Ok(())
    }

    /// Dispatch events that arrive within `timeout`.
    pub fn dispatch_timeout(&mut self, timeout: std::time::Duration) -> Result<()> {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        self.queue.flush()?;
        self.queue.dispatch_pending(&mut self.state)?;
        let Some(guard) = self.queue.prepare_read() else {
            return Ok(());
        };
        let ts = Timespec {
            tv_sec: timeout.as_secs() as i64,
            tv_nsec: i64::from(timeout.subsec_nanos()),
        };
        let ready = {
            let fd = guard.connection_fd();
            let mut fds = [PollFd::new(&fd, PollFlags::IN)];
            poll(&mut fds, Some(&ts)).context("poll")? > 0
        };
        if ready {
            match guard.read() {
                Ok(_) => {}
                Err(wayland_client::backend::WaylandError::Io(e))
                    if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.into()),
            }
        }
        self.queue.dispatch_pending(&mut self.state)?;
        Ok(())
    }

    /// Send everything and wait until the compositor has handled it.
    pub fn sync(&mut self) -> Result<()> {
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }
}

/// Milliseconds for event timestamps.
pub fn now_ms() -> u32 {
    let t = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    (t.tv_sec as u64 * 1000 + t.tv_nsec as u64 / 1_000_000) as u32
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(State: ignore wl_seat::WlSeat);
delegate_noop!(State: ZwlrVirtualPointerManagerV1);
delegate_noop!(State: ZwlrVirtualPointerV1);
delegate_noop!(State: ZwpVirtualKeyboardManagerV1);
delegate_noop!(State: ZwpVirtualKeyboardV1);
delegate_noop!(State: ExtIdleNotifierV1);

impl Dispatch<ExtIdleNotificationV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => state.idled = true,
            ext_idle_notification_v1::Event::Resumed => state.idled = false,
            _ => {}
        }
    }
}
