//! The microphone, through GStreamer from PipeWire or PulseAudio: 16 kHz
//! mono, handed on in 30 ms reads as the voice pipeline takes them. Open
//! only while Ari is listening.

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gtk4::glib;
use tracing::warn;

/// 30 ms at 16 kHz, as Android reads.
const READ: usize = 480;

const PIPELINE: &str = "autoaudiosrc ! audioconvert ! audioresample \
    ! audio/x-raw,format=S16LE,rate=16000,channels=1,layout=interleaved \
    ! appsink name=sink sync=false";

pub struct Mic {
    pipeline: gst::Pipeline,
    _watch: gst::bus::BusWatchGuard,
}

impl Mic {
    /// Start reading the default microphone. `push` gets each read, on
    /// GStreamer's thread.
    pub fn open(push: impl Fn(&[i16]) + Send + 'static) -> Result<Self, glib::Error> {
        gst::init()?;
        let pipeline = gst::parse::launch(PIPELINE)?
            .downcast::<gst::Pipeline>()
            .map_err(|_| glib::Error::new(gst::CoreError::Failed, "not a pipeline"))?;
        let sink = pipeline
            .by_name("sink")
            .and_then(|sink| sink.downcast::<gst_app::AppSink>().ok())
            .ok_or_else(|| glib::Error::new(gst::CoreError::Failed, "no appsink"))?;
        let mut pending: Vec<i16> = Vec::with_capacity(READ * 2);
        sink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                    let bytes = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                    pending.extend(
                        bytes
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .map(|pair| i16::from_le_bytes(*pair)),
                    );
                    let whole = pending.len() / READ * READ;
                    for read in pending[..whole].chunks(READ) {
                        push(read);
                    }
                    pending.drain(..whole);
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
        let bus = pipeline
            .bus()
            .ok_or_else(|| glib::Error::new(gst::CoreError::Failed, "no bus"))?;
        let watch = bus
            .add_watch_local(|_, message| {
                if let gst::MessageView::Error(error) = message.view() {
                    warn!(error = %error.error(), debug = ?error.debug(), "the microphone failed");
                }
                glib::ControlFlow::Continue
            })
            .map_err(|error| glib::Error::new(gst::CoreError::Failed, &error.to_string()))?;
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|error| glib::Error::new(gst::CoreError::StateChange, &error.to_string()))?;
        Ok(Self {
            pipeline,
            _watch: watch,
        })
    }
}

impl Drop for Mic {
    fn drop(&mut self) {
        if let Err(error) = self.pipeline.set_state(gst::State::Null) {
            warn!(%error, "the microphone wouldn't close");
        }
    }
}
