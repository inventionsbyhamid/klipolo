//! The network path from `NWPathMonitor` (cidre's `nw`): one monitor for the process, on a serial utility queue, for
//! the rest of its life. It observes the interfaces and sends nothing.

use cidre::{dispatch, nw};

use crate::network::NetPath;

pub(crate) fn watch() {
    let attr = dispatch::Attr::serial_with_qos_class(dispatch::QosClass::UTILITY, 0);
    let queue =
        dispatch::Queue::with_label_and_attrs(Some(c"com.klipolo.network-path"), Some(&attr));
    let mut monitor = nw::PathMonitor::new();
    monitor.set_queue(&queue);
    monitor.set_update_handler(|path: &nw::Path| crate::network::report(read(path)));
    monitor.start();
    // Dropping the monitor would cancel it; it keeps its queue.
    std::mem::forget(monitor);
}

/// Online when the path is satisfied (a usable route now); Low Data Mode is `isConstrained`, and cellular or a
/// personal hotspot `isExpensive`.
fn read(path: &nw::Path) -> NetPath {
    NetPath {
        online: matches!(path.status(), nw::PathStatus::Satisfied),
        constrained: path.is_constrained(),
        expensive: path.is_expensive(),
    }
}
