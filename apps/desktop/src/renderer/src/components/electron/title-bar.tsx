import { useEffect, useState } from "react";
import { Box, IconButton } from "@chakra-ui/react";
import {
  FiMinus,
  FiMaximize2,
  FiMinimize2,
  FiX,
  FiChevronsDown,
} from "react-icons/fi";
import { layoutStyles } from "@/layout";

function TitleBar(): JSX.Element {
  const [isMaximized, setIsMaximized] = useState(false);
  const [isFullScreen, setIsFullScreen] = useState(false);
  const isMac = window.electron?.process.platform === "darwin";

  useEffect(() => {
    if (!window.api) return undefined;
    const unsubscribeMaximized =
      window.api.onWindowMaximizedChange(setIsMaximized);
    const unsubscribeFullscreen =
      window.api.onWindowFullscreenChange(setIsFullScreen);

    return () => {
      unsubscribeMaximized();
      unsubscribeFullscreen();
    };
  }, []);

  const handleMaximizeClick = () => {
    if (isFullScreen) {
      window.api?.unfullscreenWindow();
    } else {
      window.api?.maximizeWindow();
    }
  };

  const getButtonLabel = () => {
    if (isFullScreen) return "Exit Full Screen";
    if (isMaximized) return "Restore";
    return "Maximize";
  };

  const getButtonIcon = () => {
    if (isFullScreen) return <FiChevronsDown />;
    if (isMaximized) return <FiMinimize2 />;
    return <FiMaximize2 />;
  };

  if (isMac) {
    return (
      <Box {...layoutStyles.macTitleBar}>
        <Box {...layoutStyles.titleBarTitle}>Open LLM VTuber</Box>
      </Box>
    );
  }

  return (
    <Box {...layoutStyles.windowsTitleBar}>
      <Box {...layoutStyles.titleBarTitle}>Open LLM VTuber</Box>
      <Box {...layoutStyles.titleBarButtons}>
        <IconButton
          {...layoutStyles.titleBarButton}
          onClick={() => window.api?.minimizeWindow()}
          aria-label="Minimize"
        >
          <FiMinus />
        </IconButton>
        <IconButton
          {...layoutStyles.titleBarButton}
          onClick={handleMaximizeClick}
          aria-label={getButtonLabel()}
        >
          {getButtonIcon()}
        </IconButton>
        <IconButton
          {...layoutStyles.closeButton}
          onClick={() => window.api?.closeWindow()}
          aria-label="Close"
        >
          <FiX />
        </IconButton>
      </Box>
    </Box>
  );
}

export default TitleBar;
