import { useEffect, useState } from "react";
import { host } from "../host";
import { type DataStreamState, type RecordingState, STREAM_STOPPED } from "./recording";

/** The recording (current, or the last one) and the TCP stream, kept current */
export function useCapture() {
  const [recording, setRecording] = useState<RecordingState | null>(null);
  const [stream, setStream] = useState<DataStreamState>(STREAM_STOPPED);

  useEffect(() => {
    let live = true;
    // An event is newer than the snapshot, whichever arrives first
    let heard = { recording: false, stream: false };
    const off = host.watchAppEvents((event) => {
      if (event.type === "recording") {
        const { type: _, ...state } = event;
        heard = { ...heard, recording: true };
        setRecording(state);
      } else {
        const { type: _, ...state } = event;
        heard = { ...heard, stream: true };
        setStream(state);
      }
    });
    host.appState().then(
      (s) => {
        if (!live) return;
        if (!heard.recording) setRecording(s.recording);
        if (!heard.stream) setStream(s.stream);
      },
      () => {},
    );
    return () => {
      live = false;
      off();
    };
  }, []);

  return { recording, stream };
}
