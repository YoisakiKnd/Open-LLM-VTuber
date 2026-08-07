import { useCallback } from "react";
import { useWebSocket } from "@/context/websocket-context";
import { useMediaCapture } from "@/hooks/utils/use-media-capture";
import {
  AUDIO_PROTOCOL_VERSION,
  AUDIO_SAMPLE_RATE,
  float32ToPcm16Le,
  gatewaySupportsAudioProtocolV1,
  splitPcm16Le,
} from "@/services/audio-protocol-v1";

export function useSendAudio() {
  const { sendMessage, sendBinary, baseUrl } = useWebSocket();
  const { captureAllMedia } = useMediaCapture();

  const sendAudioPartition = useCallback(
    async (audio: Float32Array) => {
      const supportsBinaryAudio = await gatewaySupportsAudioProtocolV1(baseUrl);
      if (supportsBinaryAudio) {
        sendMessage({
          type: "audio-start",
          version: AUDIO_PROTOCOL_VERSION,
          encoding: "pcm_s16le",
          sample_rate: AUDIO_SAMPLE_RATE,
          channels: 1,
          mode: "manual",
        });
        const pcm = float32ToPcm16Le(audio);
        for (const chunk of splitPcm16Le(pcm)) sendBinary(chunk);
        const images = await captureAllMedia();
        sendMessage({
          type: "audio-end",
          version: AUDIO_PROTOCOL_VERSION,
          images,
        });
        return;
      }

      const chunkSize = 4096;
      for (let index = 0; index < audio.length; index += chunkSize) {
        const endIndex = Math.min(index + chunkSize, audio.length);
        sendMessage({
          type: "mic-audio-data",
          audio: Array.from(audio.slice(index, endIndex)),
        });
      }
      const images = await captureAllMedia();
      sendMessage({ type: "mic-audio-end", images });
    },
    [baseUrl, captureAllMedia, sendBinary, sendMessage],
  );

  return { sendAudioPartition };
}
