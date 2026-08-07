import { Box, Image } from "@chakra-ui/react";
import { memo, useEffect, useRef } from "react";
import { canvasStyles } from "./canvas-styles";
import { useCamera } from "@/context/camera-context";
import { useBgUrl } from "@/context/bgurl-context";

const Background = memo(({ children }: { children?: React.ReactNode }) => {
  const videoRef = useRef<HTMLVideoElement>(null);
  const {
    backgroundStream,
    isBackgroundStreaming,
    startBackgroundCamera,
    stopBackgroundCamera,
  } = useCamera();
  const { useCameraBackground, setUseCameraBackground, backgroundUrl } =
    useBgUrl();

  useEffect(() => {
    if (useCameraBackground && !isBackgroundStreaming) {
      void startBackgroundCamera().catch(() => setUseCameraBackground(false));
    } else if (!useCameraBackground && isBackgroundStreaming) {
      stopBackgroundCamera();
    }
  }, [
    useCameraBackground,
    isBackgroundStreaming,
    startBackgroundCamera,
    stopBackgroundCamera,
    setUseCameraBackground,
  ]);

  useEffect(() => {
    if (videoRef.current && backgroundStream) {
      videoRef.current.srcObject = backgroundStream;
    }
  }, [backgroundStream]);

  return (
    <Box {...canvasStyles.background.container}>
      {useCameraBackground ? (
        <video
          ref={videoRef}
          autoPlay
          playsInline
          muted
          style={{
            ...canvasStyles.background.video,
            display: isBackgroundStreaming ? "block" : "none",
            transform: "scaleX(-1)",
          }}
        />
      ) : (
        <Image
          {...canvasStyles.background.image}
          src={backgroundUrl}
          alt="background"
        />
      )}
      {children}
    </Box>
  );
});

Background.displayName = "Background";

export default Background;
