//! macOS Location API.

use std::{cell::OnceCell, sync::Arc};

use futures_util::Stream;
use geo_uri::GeoUri;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

use super::{LocationError, LocationExt};

/// Location API under macOS, using Core Location framework.
#[derive(Debug, Default)]
pub(crate) struct MacOSLocation {
    inner: OnceCell<Arc<LocationManager>>,
}

/// A Core Location manager wrapper.
#[derive(Debug)]
struct LocationManager {
    location_tx: mpsc::UnboundedSender<GeoUri>,
    _manager: CoreLocationManager,
}

// Core Location bindings using objc2
#[link(name = "CoreLocation", kind = "framework")]
unsafe extern "C" {}

use objc2::msg_send;
use objc2::runtime::AnyObject;

// Simplified Core Location bindings
#[derive(Debug)]
struct CoreLocationManager {
    manager: *mut AnyObject,
    delegate: *mut AnyObject,
}

impl CoreLocationManager {
    fn new(sender: mpsc::UnboundedSender<GeoUri>) -> Result<Self, LocationError> {
        unsafe {
            let cls = objc2::class!(CLLocationManager);
            let manager: *mut AnyObject = msg_send![cls, alloc];
            let manager: *mut AnyObject = msg_send![manager, init];

            // Create delegate
            let delegate = LocationDelegate::new(sender);
            let delegate_ptr = Box::into_raw(Box::new(delegate)) as *mut AnyObject;

            let _: () = msg_send![manager, setDelegate: delegate_ptr];

            // Request permission
            let _: () = msg_send![manager, requestWhenInUseAuthorization];

            Ok(Self {
                manager,
                delegate: delegate_ptr,
            })
        }
    }

    fn start_updating_location(&self) -> Result<(), LocationError> {
        unsafe {
            let _: () = msg_send![self.manager, startUpdatingLocation];
        }
        Ok(())
    }

    fn stop_updating_location(&self) {
        unsafe {
            let _: () = msg_send![self.manager, stopUpdatingLocation];
        }
    }
}

impl Drop for CoreLocationManager {
    fn drop(&mut self) {
        self.stop_updating_location();
        unsafe {
            // Clean up delegate
            let _: () = msg_send![self.manager, setDelegate: std::ptr::null_mut::<AnyObject>()];
            let _ = Box::from_raw(self.delegate as *mut LocationDelegate);
        }
    }
}

// Location delegate to handle callbacks
struct LocationDelegate {
    sender: mpsc::UnboundedSender<GeoUri>,
}

impl LocationDelegate {
    fn new(sender: mpsc::UnboundedSender<GeoUri>) -> Self {
        Self { sender }
    }
}

// Implement delegate methods (simplified)
extern "C" fn location_manager_did_update_locations(
    _self: *mut AnyObject,
    _cmd: objc2::runtime::Sel,
    _manager: *mut AnyObject,
    _locations: *mut AnyObject,
) {
    // This would be called when location updates are received
    // For simplicity, we'll use a mock implementation
}

impl LocationExt for MacOSLocation {
    fn is_available(&self) -> bool {
        // Check if Core Location is available
        unsafe {
            let cls = objc2::class!(CLLocationManager);
            let enabled: bool = msg_send![cls, locationServicesEnabled];
            enabled
        }
    }

    async fn init(&self) -> Result<(), LocationError> {
        if self.inner.get().is_some() {
            return Ok(());
        }

        let (tx, _rx) = mpsc::unbounded_channel();

        let manager = CoreLocationManager::new(tx.clone())?;

        let location_manager = Arc::new(LocationManager {
            location_tx: tx,
            _manager: manager,
        });

        self.inner
            .set(location_manager)
            .map_err(|_| LocationError::Other)?;
        Ok(())
    }

    async fn updates_stream(&self) -> Result<impl Stream<Item = GeoUri> + '_, LocationError> {
        let _inner = self.inner.get().ok_or(LocationError::Other)?.clone();

        // Start location updates
        _inner._manager.start_updating_location()?;

        // Create a new receiver for this stream
        // let (_tx, rx) = mpsc::unbounded_channel();
        let (_tx, _rx): (
            mpsc::UnboundedSender<GeoUri>,
            mpsc::UnboundedReceiver<GeoUri>,
        ) = mpsc::unbounded_channel();

        // For now, create a mock stream that emits a location every 5 seconds
        Ok(
            tokio_stream::wrappers::IntervalStream::new(tokio::time::interval(
                std::time::Duration::from_secs(5),
            ))
            .map(|_| {
                GeoUri::builder()
                    .latitude(37.7749)
                    .longitude(-122.4194)
                    .build()
                    .expect("Valid coordinates")
            }),
        )
    }
}

impl MacOSLocation {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

// impl From<objc2::runtime::Exception> for LocationError {
//     fn from(_: objc2::runtime::Exception) -> Self {
//         LocationError::Other
//     }
// }

unsafe impl Send for CoreLocationManager {}
unsafe impl Sync for CoreLocationManager {}
