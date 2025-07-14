use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt,
    future::IntoFuture,
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use futures_util::future::BoxFuture;
use gtk::glib;
use matrix_sdk::{
    Client,
    media::{MediaRequestParameters, UniqueKey},
};
use tokio::{
    sync::{Mutex as AsyncMutex, broadcast},
    task::AbortHandle,
};
use tracing::{debug, warn};

use super::{Image, ImageError, load_image};
use crate::{
    spawn_tokio,
    utils::{
        File,
        media::{FrameDimensions, MediaFileError},
        save_data_to_tmp_file,
    },
};

/// The default image request queue.
pub(crate) static IMAGE_QUEUE: LazyLock<ImageRequestQueue> = LazyLock::new(ImageRequestQueue::new);

/// The default limit of the [`ImageRequestQueue`], aka the maximum number of
/// concurrent image requests.
const DEFAULT_QUEUE_LIMIT: usize = 20;
/// The maximum number of retries for a single request.
const MAX_REQUEST_RETRY_COUNT: u8 = 2;
/// The time after which a request is considered to be stalled, 10
/// seconds.
const STALLED_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A queue for image requests.
///
/// This implements the following features:
/// - Limit the number of concurrent requests,
/// - Prioritize requests according to their importance,
/// - Avoid duplicate requests,
/// - Watch requests that fail with I/O errors to:
///   - Reinsert them at the end of the queue to retry them later,
///   - Reduce the pool capacity temporarily to avoid more similar errors and
///     let the system recover.
/// - Watch requests that take too long to:
///   - Log them,
///   - Ignore them in the count of ongoing requests.
pub(crate) struct ImageRequestQueue {
    inner: Arc<AsyncMutex<ImageRequestQueueInner>>,
}

struct ImageRequestQueueInner {
    /// The current limit of the ongoing requests count.
    ///
    /// This may change if an error is encountered, to let the system recover.
    limit: usize,
    /// The image requests in the queue.
    requests: HashMap<ImageRequestId, ImageRequest>,
    /// The ongoing requests.
    ongoing: HashSet<ImageRequestId>,
    /// The stalled requests.
    stalled: HashSet<ImageRequestId>,
    /// The queue of requests with default priority.
    queue_default: VecDeque<ImageRequestId>,
    /// The queue of requests with low priority.
    queue_low: VecDeque<ImageRequestId>,
}

impl ImageRequestQueue {
    /// Construct an empty `ImageRequestQueue` with the default settings.
    fn new() -> Self {
        Self {
            inner: AsyncMutex::new(ImageRequestQueueInner {
                limit: DEFAULT_QUEUE_LIMIT,
                requests: Default::default(),
                ongoing: Default::default(),
                stalled: Default::default(),
                queue_default: Default::default(),
                queue_low: Default::default(),
            })
            .into(),
        }
    }

    /// Add a request to download an image.
    ///
    /// If another request for the same image already exists, this will reuse
    /// the same request.
    pub(crate) async fn add_download_request(
        &self,
        client: Client,
        settings: MediaRequestParameters,
        dimensions: Option<FrameDimensions>,
        priority: ImageRequestPriority,
    ) -> ImageRequestHandle {
        let inner = self.inner.clone();
        spawn_tokio!(async move {
            inner
                .lock()
                .await
                .add_download_request(client, settings, dimensions, priority)
        })
        .await
        .expect("task was not aborted")
    }

    /// Add a request to load an image from a file.
    ///
    /// If another request for the same file already exists, this will reuse the
    /// same request.
    pub(crate) async fn add_file_request(
        &self,
        file: File,
        dimensions: Option<FrameDimensions>,
    ) -> ImageRequestHandle {
        let inner = self.inner.clone();
        spawn_tokio!(async move { inner.lock().await.add_file_request(file, dimensions) })
            .await
            .expect("task was not aborted")
    }

    /// Mark the request with the given ID as stalled.
    async fn mark_as_stalled(&self, request_id: ImageRequestId) {
        self.inner.lock().await.mark_as_stalled(request_id);
    }

    /// Retry the request with the given ID.
    ///
    /// If `lower_limit` is `true`, we will also lower the limit of the queue.
    async fn retry_request(&self, request_id: &ImageRequestId, lower_limit: bool) {
        self.inner
            .lock()
            .await
            .retry_request(request_id, lower_limit);
    }

    /// Remove the request with the given ID.
    async fn remove_request(&self, request_id: &ImageRequestId) {
        self.inner.lock().await.remove_request(request_id);
    }
}

impl ImageRequestQueueInner {
    /// Whether we have reache the current limit of concurrent requests.
    fn is_limit_reached(&self) -> bool {
        self.ongoing.len() >= self.limit
    }

    /// Add the given request to the queue.
    fn add_request(&mut self, request_id: ImageRequestId, request: ImageRequest) {
        let is_limit_reached = self.is_limit_reached();
        if !is_limit_reached || request.priority == ImageRequestPriority::High {
            // Spawn the request right away.
            self.ongoing.insert(request_id.clone());
            request.spawn();
        } else {
            // Queue the request.
            let queue = if request.priority == ImageRequestPriority::Default {
                &mut self.queue_default
            } else {
                &mut self.queue_low
            };

            queue.push_back(request_id.clone());
        }
        self.requests.insert(request_id, request);
    }

    /// Add a request to download an image.
    ///
    /// If another request for the same image already exists, this will reuse
    /// the same request.
    fn add_download_request(
        &mut self,
        client: Client,
        settings: MediaRequestParameters,
        dimensions: Option<FrameDimensions>,
        priority: ImageRequestPriority,
    ) -> ImageRequestHandle {
        let data = DownloadRequestData {
            client,
            settings,
            dimensions,
        };
        let request_id = data.request_id();

        // If the request already exists, use the existing one.
        if let Some(request) = self.requests.get(&request_id) {
            let result_receiver = request.result_sender.subscribe();
            return ImageRequestHandle::new(result_receiver);
        }

        // Build and add the request.
        let (request, result_receiver) = ImageRequest::new(data, priority);
        self.add_request(request_id.clone(), request);

        ImageRequestHandle::new(result_receiver)
    }

    /// Add a request to load an image from a file.
    ///
    /// If another request for the same file already exists, this will reuse the
    /// same request.
    fn add_file_request(
        &mut self,
        file: File,
        dimensions: Option<FrameDimensions>,
    ) -> ImageRequestHandle {
        let data = FileRequestData { file, dimensions };
        let request_id = data.request_id();

        // If the request already exists, use the existing one.
        if let Some(request) = self.requests.get(&request_id) {
            let result_receiver = request.result_sender.subscribe();
            return ImageRequestHandle::new(result_receiver);
        }

        // Build and add the request.
        // Always use high priority because file requests should always be for
        // previewing a local image.
        let (request, result_receiver) = ImageRequest::new(data, ImageRequestPriority::High);

        self.add_request(request_id.clone(), request);

        ImageRequestHandle::new(result_receiver)
    }

    /// Mark the request with the given ID as stalled.
    fn mark_as_stalled(&mut self, request_id: ImageRequestId) {
        self.ongoing.remove(&request_id);
        self.stalled.insert(request_id);

        self.spawn_next();
    }

    /// Retry the request with the given ID.
    ///
    /// If `lower_limit` is `true`, we will also lower the limit of the queue.
    fn retry_request(&mut self, request_id: &ImageRequestId, lower_limit: bool) {
        self.ongoing.remove(request_id);

        if lower_limit {
            // Only one request at a time until the problem is likely fixed.
            self.limit = 1;
        }

        let is_limit_reached = self.is_limit_reached();

        match self.requests.get_mut(request_id) {
            Some(request) => {
                request.retries_count += 1;

                // For fairness, only re-spawn the request right away if there is no other
                // request waiting with a priority higher or equal to this one.
                let can_spawn_request = if request.priority == ImageRequestPriority::High {
                    true
                } else {
                    !is_limit_reached
                        && self.queue_default.is_empty()
                        && (request.priority == ImageRequestPriority::Default
                            || self.queue_low.is_empty())
                };

                if can_spawn_request {
                    // Re-spawn the request right away.
                    self.ongoing.insert(request_id.clone());
                    request.spawn();
                } else {
                    // Queue the request.
                    let queue = if request.priority == ImageRequestPriority::Default {
                        &mut self.queue_default
                    } else {
                        &mut self.queue_low
                    };

                    queue.push_back(request_id.clone());
                }
            }
            None => {
                // This should not happen.
                warn!("Could not find request {request_id} to retry");
            }
        }

        self.spawn_next();
    }

    /// Remove the request with the given ID.
    fn remove_request(&mut self, request_id: &ImageRequestId) {
        self.ongoing.remove(request_id);
        self.stalled.remove(request_id);
        self.queue_default.retain(|id| id != request_id);
        self.queue_low.retain(|id| id != request_id);
        self.requests.remove(request_id);

        self.spawn_next();
    }

    /// Spawn as many requests as possible.
    fn spawn_next(&mut self) {
        while !self.is_limit_reached() {
            let Some(request_id) = self
                .queue_default
                .pop_front()
                .or_else(|| self.queue_low.pop_front())
            else {
                // No request to spawn.
                return;
            };
            let Some(request) = self.requests.get(&request_id) else {
                // The queues and requests are out of sync, this should not happen.
                warn!("Missing image request {request_id}");
                continue;
            };

            self.ongoing.insert(request_id.clone());
            request.spawn();
        }

        // If there are no ongoing requests, restore the limit to its default value.
        if self.ongoing.is_empty() {
            self.limit = DEFAULT_QUEUE_LIMIT;
        }
    }
}

/// A request for an image.
struct ImageRequest {
    /// The data of the request.
    data: ImageRequestData,
    /// The priority of the request.
    priority: ImageRequestPriority,
    /// The sender of the channel to use to send the result.
    result_sender: broadcast::Sender<Result<Image, ImageError>>,
    /// The number of retries for this request.
    retries_count: u8,
    /// The handle for aborting the current task of this request.
    task_handle: Arc<Mutex<Option<AbortHandle>>>,
    /// The timeout source for marking this request as stalled.
    stalled_timeout_source: Arc<Mutex<Option<glib::SourceId>>>,
}

impl ImageRequest {
    /// Construct an image request with the given data and priority.
    fn new(
        data: impl Into<ImageRequestData>,
        priority: ImageRequestPriority,
    ) -> (Self, broadcast::Receiver<Result<Image, ImageError>>) {
        let (result_sender, result_receiver) = broadcast::channel(1);
        (
            Self {
                data: data.into(),
                priority,
                result_sender,
                retries_count: 0,
                task_handle: Default::default(),
                stalled_timeout_source: Default::default(),
            },
            result_receiver,
        )
    }

    /// Whether we can retry a request with the given retries count and after
    /// the given error.
    fn can_retry(retries_count: u8, error: ImageError) -> bool {
        // Retry if we have not the max retry count && if it's a glycin error.
        // We assume that the download requests have already been retried by the client.
        retries_count < MAX_REQUEST_RETRY_COUNT && error == ImageError::Unknown
    }

    /// Spawn this request.
    fn spawn(&self) {
        let data = self.data.clone();
        let result_sender = self.result_sender.clone();
        let retries_count = self.retries_count;
        let task_handle = self.task_handle.clone();
        let stalled_timeout_source = self.stalled_timeout_source.clone();

        let abort_handle = spawn_tokio!(async move {
            let request_id = data.request_id();

            let stalled_timeout_source_clone = stalled_timeout_source.clone();
            let request_id_clone = request_id.clone();
            let source = glib::timeout_add_once(STALLED_REQUEST_TIMEOUT, move || {
                spawn_tokio!(async move {
                    // Drop the timeout source.
                    let _ = stalled_timeout_source_clone.lock().map(|mut s| s.take());

                    IMAGE_QUEUE.mark_as_stalled(request_id_clone.clone()).await;
                    debug!("Request {request_id_clone} is taking longer than {} seconds, it is now marked as stalled", STALLED_REQUEST_TIMEOUT.as_secs());
                });
            });
            if let Ok(Some(source)) = stalled_timeout_source.lock().map(|mut s| s.replace(source)) {
                // This should not happen, but cancel the old timeout if we have one.
                source.remove();
            }

            let result = data.await;

            // Cancel the timeout.
            if let Ok(Some(source)) = stalled_timeout_source.lock().map(|mut s| s.take()) {
                source.remove();
            }

            // Now that we have the result, do not offer to abort the task anymore.
            let _ = task_handle.lock().map(|mut s| s.take());

            // If it is an error, maybe we can retry it.
            if let Some(error) = result
                .as_ref()
                .err()
                .filter(|error| Self::can_retry(retries_count, **error))
            {
                // Lower the limit of the queue if it is an I/O error, usually it means that glycin cannot spawn a sandbox.
                let lower_limit = *error == ImageError::Io;
                IMAGE_QUEUE
                    .retry_request(&request_id, lower_limit)
                    .await;
                return;
            }

            // Send the result.
            if let Err(error) = result_sender.send(result) {
                warn!("Could not send result of image request {request_id}: {error}");
            }
            IMAGE_QUEUE.remove_request(&request_id).await;
        }).abort_handle();

        if let Ok(Some(handle)) = self.task_handle.lock().map(|mut s| s.replace(abort_handle)) {
            // This should not happen, but cancel the old task if we have one.
            handle.abort();
        }
    }
}

impl Drop for ImageRequest {
    fn drop(&mut self) {
        if let Ok(Some(source)) = self.stalled_timeout_source.lock().map(|mut s| s.take()) {
            source.remove();
        }
        if let Ok(Some(handle)) = self.task_handle.lock().map(|mut s| s.take()) {
            handle.abort();

            // Broadcast that the request was aborted.
            let request_id = self.data.request_id();
            let result_sender = self.result_sender.clone();
            spawn_tokio!(async move {
                if let Err(error) = result_sender.send(Err(ImageError::Aborted)) {
                    warn!("Could not abort image request {request_id}: {error}");
                }
            });
        }
    }
}

/// The data of a request to download an image.
#[derive(Clone)]
struct DownloadRequestData {
    /// The Matrix client to use to make the request.
    client: Client,
    /// The settings of the request.
    settings: MediaRequestParameters,
    /// The dimensions to request.
    dimensions: Option<FrameDimensions>,
}

impl DownloadRequestData {
    /// The ID of the image request with this data.
    fn request_id(&self) -> ImageRequestId {
        ImageRequestId::Download(self.settings.unique_key())
    }
}

impl IntoFuture for DownloadRequestData {
    type Output = Result<File, MediaFileError>;
    type IntoFuture = BoxFuture<'static, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        let Self {
            client, settings, ..
        } = self;

        Box::pin(async move {
            let media = client.media();
            let data = match media.get_media_content(&settings, true).await {
                Ok(data) => data,
                Err(error) => {
                    return Err(MediaFileError::from(error));
                }
            };

            let file = save_data_to_tmp_file(data).await?;
            Ok(file)
        })
    }
}

/// The data of a request to load an image file into a paintable.
#[derive(Clone)]
struct FileRequestData {
    /// The image file to load.
    file: File,
    /// The dimensions to request.
    dimensions: Option<FrameDimensions>,
}

impl FileRequestData {
    /// The ID of the image request with this data.
    fn request_id(&self) -> ImageRequestId {
        ImageRequestId::File(self.file.path().expect("file has a path"))
    }
}

impl IntoFuture for FileRequestData {
    #[cfg(target_os = "linux")]
    type Output = Result<Image, glycin::ErrorCtx>;
    #[cfg(target_os = "macos")]
    type Output = Result<Image, ImageError>;
    type IntoFuture = BoxFuture<'static, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        let Self { file, dimensions } = self;

        Box::pin(async move { load_image(file, dimensions).await })
    }
}

/// The data of an image request.
#[derive(Clone)]
enum ImageRequestData {
    /// The data for a download request.
    Download {
        /// The data to download the image.
        download_data: DownloadRequestData,
        /// The data to load the image into a paintable, after it was
        /// downloaded.
        file_data: Option<FileRequestData>,
    },
    /// The data for a file request.
    File(FileRequestData),
}

impl ImageRequestData {
    /// The ID of the image request with this data.
    fn request_id(&self) -> ImageRequestId {
        match self {
            ImageRequestData::Download { download_data, .. } => download_data.request_id(),
            ImageRequestData::File(file_data) => file_data.request_id(),
        }
    }

    /// The data for the next request with this image request data.
    fn into_next_request_data(self) -> DownloadOrFileRequestData {
        match self {
            Self::Download {
                download_data,
                file_data,
            } => {
                if let Some(file_data) = file_data {
                    file_data.into()
                } else {
                    download_data.into()
                }
            }
            Self::File(file_data) => file_data.into(),
        }
    }
}

impl IntoFuture for ImageRequestData {
    type Output = Result<Image, ImageError>;
    type IntoFuture = BoxFuture<'static, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let file_data = match self.into_next_request_data() {
                DownloadOrFileRequestData::Download(download_data) => {
                    let dimensions = download_data.dimensions;

                    // Download the image to a file.
                    match download_data.await {
                        Ok(file) => FileRequestData { file, dimensions },
                        Err(error) => {
                            warn!("Could not retrieve image: {error}");
                            return Err(error.into());
                        }
                    }
                }
                DownloadOrFileRequestData::File(file_data) => file_data,
            };

            // Load the image from the file.
            match file_data.clone().await {
                Ok(image) => Ok(image),
                Err(error) => {
                    warn!("Could not load image from file: {error}");
                    Err(error.into())
                }
            }
        })
    }
}

impl From<DownloadRequestData> for ImageRequestData {
    fn from(download_data: DownloadRequestData) -> Self {
        Self::Download {
            download_data,
            file_data: None,
        }
    }
}

impl From<FileRequestData> for ImageRequestData {
    fn from(value: FileRequestData) -> Self {
        Self::File(value)
    }
}

/// The data of a download request or a file request.
#[derive(Clone)]
enum DownloadOrFileRequestData {
    /// The data for a download request.
    Download(DownloadRequestData),
    /// The data for a file request.
    File(FileRequestData),
}

impl From<DownloadRequestData> for DownloadOrFileRequestData {
    fn from(download_data: DownloadRequestData) -> Self {
        Self::Download(download_data)
    }
}

impl From<FileRequestData> for DownloadOrFileRequestData {
    fn from(value: FileRequestData) -> Self {
        Self::File(value)
    }
}

/// A unique identifier for an image request.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
enum ImageRequestId {
    /// The identifier for a download request.
    Download(String),
    /// The identifier for a file request.
    File(PathBuf),
}

impl fmt::Display for ImageRequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Download(id) => id.fmt(f),
            Self::File(path) => path.to_string_lossy().fmt(f),
        }
    }
}

/// The priority of an image request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ImageRequestPriority {
    /// The highest priority.
    ///
    /// A request with this priority will be spawned right away and will not be
    /// limited by the capacity of the pool.
    ///
    /// Should be used for images presented in the image viewer, the user avatar
    /// in the account settings or the room avatar in the room details.
    High,
    /// The default priority.
    ///
    /// Should be used for images in messages in the room history, or in the
    /// media history.
    #[default]
    Default,
    /// The lowest priority.
    ///
    /// Should be used for avatars in the sidebar, the room history or the
    /// members list.
    Low,
}

/// A handle for `await`ing an image request.
pub(crate) struct ImageRequestHandle {
    receiver: broadcast::Receiver<Result<Image, ImageError>>,
}

impl ImageRequestHandle {
    /// Construct a new `ImageRequestHandle` with the given request ID.
    fn new(receiver: broadcast::Receiver<Result<Image, ImageError>>) -> Self {
        Self { receiver }
    }
}

impl IntoFuture for ImageRequestHandle {
    type Output = Result<Image, ImageError>;
    type IntoFuture = BoxFuture<'static, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        let mut receiver = self.receiver;
        Box::pin(async move {
            let handle = spawn_tokio!(async move { receiver.recv().await });
            match handle.await.expect("task was not aborted") {
                Ok(Ok(image)) => Ok(image),
                Ok(err) => err,
                Err(error) => {
                    warn!("Could not load image: {error}");
                    Err(ImageError::Unknown)
                }
            }
        })
    }
}
