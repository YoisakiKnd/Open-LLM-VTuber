import { useEffect, useCallback } from "react";
import { useInterrupt } from "@/components/canvas/live2d";
import { useMicToggle } from "./use-mic-toggle";
import { useLive2DConfig } from "@/context/live2d-config-context";
import { useSwitchCharacter } from "@/hooks/utils/use-switch-character";
import { useForceIgnoreMouse } from "@/hooks/utils/use-force-ignore-mouse";
import { useMode } from "@/context/mode-context";

export function useIpcHandlers() {
  const { handleMicToggle } = useMicToggle();
  const { interrupt } = useInterrupt();
  const { modelInfo, setModelInfo } = useLive2DConfig();
  const { switchCharacter } = useSwitchCharacter();
  const { setForceIgnoreMouse } = useForceIgnoreMouse();
  const { mode } = useMode();
  const isPet = mode === "pet";

  const micToggleHandler = useCallback(() => {
    handleMicToggle();
  }, [handleMicToggle]);

  const interruptHandler = useCallback(() => {
    interrupt();
  }, [interrupt]);

  const scrollToResizeHandler = useCallback(() => {
    if (modelInfo) {
      setModelInfo({
        ...modelInfo,
        scrollToResize: !modelInfo.scrollToResize,
      });
    }
  }, [modelInfo, setModelInfo]);

  const switchCharacterHandler = useCallback(
    (filename: string) => {
      switchCharacter(filename);
    },
    [switchCharacter],
  );

  // Handler for force ignore mouse state changes from main process
  const forceIgnoreMouseChangedHandler = useCallback(
    (isForced: boolean) => {
      console.log("Force ignore mouse changed:", isForced);
      setForceIgnoreMouse(isForced);
    },
    [setForceIgnoreMouse],
  );

  // Handle toggle force ignore mouse from menu
  const toggleForceIgnoreMouseHandler = useCallback(() => {
    window.api.toggleForceIgnoreMouse();
  }, []);

  useEffect(() => {
    if (!window.api || !isPet) return;

    const unsubscribeMic = window.api.onMicToggle(micToggleHandler);
    const unsubscribeInterrupt = window.api.onInterrupt(interruptHandler);
    const unsubscribeScroll = window.api.onToggleScrollToResize(
      scrollToResizeHandler,
    );
    const unsubscribeCharacter = window.api.onSwitchCharacter(
      switchCharacterHandler,
    );
    const unsubscribeToggleForce = window.api.onToggleForceIgnoreMouse(
      toggleForceIgnoreMouseHandler,
    );
    const unsubscribeForceChanged = window.api.onForceIgnoreMouseChanged(
      forceIgnoreMouseChangedHandler,
    );

    return () => {
      unsubscribeMic();
      unsubscribeInterrupt();
      unsubscribeScroll();
      unsubscribeCharacter();
      unsubscribeToggleForce();
      unsubscribeForceChanged();
    };
  }, [
    micToggleHandler,
    interruptHandler,
    scrollToResizeHandler,
    switchCharacterHandler,
    toggleForceIgnoreMouseHandler,
    forceIgnoreMouseChangedHandler,
    isPet,
  ]);
}
