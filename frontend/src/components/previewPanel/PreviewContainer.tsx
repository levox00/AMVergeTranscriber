import VideoPlayer from "./videoPlayer/VideoPlayer.tsx"
import HowToUse from "./HowToUse.tsx"
import React from "react";
import {
  FaFolderOpen,
  FaFileExport,
  FaPencilAlt,
} from "react-icons/fa";
import Dropdown from "../common/Dropdown";
import { ClipItem } from "../../types/domain";
import { useThemeSettingsStore } from "../../stores/settingsStore";

import { useAppStateStore } from "../../stores/appStore.ts";
import { useAppPersistedStore } from "../../stores/appStore.ts";
import { useUIStateStore } from "../../stores/UIStore.ts";
import { useGeneralSettingsStore } from "../../stores/settingsStore.ts";
import useImportExport from "../../hooks/useImportExport";
import { renderProfileIcon } from "../../features/export/profileIconUtils.tsx";
import {
  getActiveExportProfile,
  getExportProfileSummary,
  supportsClipMerge,
} from "../../features/export/profiles.ts";
type PreviewContainerProps = {
  sourceClipSrc: string | null;         // File path for video
  sourceClipId: string | null;     
  sourceClipThumbnail: string | null;
  onTimeUpdate?: (time: number) => void;
};

export default function PreviewContainer(props: PreviewContainerProps) {
  const [showMergeNameModal, setShowMergeNameModal] = React.useState(false);
  const mergeNameInputRef = React.useRef<HTMLInputElement | null>(null);

  const clips = useAppStateStore(s => s.clips);
  const selectedClips = useAppStateStore(s => s.selectedClips);

  const transcriptionEnabled = useThemeSettingsStore((s) => s.transcriptionEnabled);
  const [key, setKey] = React.useState(0);

  const [selectedClip, setSelectedClip] = React.useState<ClipItem | null>(null);

  // Look up clip by ID when clips load or sourceClip changes
  React.useEffect(() => {
    if (props.sourceClipId && clips.length > 0) {
      const clip = clips.find(c => c.id === props.sourceClipId);
      setSelectedClip(clip || null); // Fix: handle undefined
      console.log('Found clip by ID:', clip);
      console.log('Clip transcription:', clip?.transcription);
    }
  }, [props.sourceClipId, clips]);
  
  const transcription = selectedClip?.transcription;
  const clipsLoaded = clips.length > 0;

  console.log('Clips loaded:', clipsLoaded);
  console.log('Looking for clip with ID:', props.sourceClipId);
  console.log('Found clip:', selectedClip);
  console.log('Transcription:', transcription);

  const videoIsHEVC = useAppStateStore(s => s.videoIsHEVC);
  const userHasHEVC = useAppStateStore(s => s.userHasHEVC);
  const importToken = useAppStateStore(s => s.importToken);
  const exportDir = useAppPersistedStore(s => s.exportDir);
  const setExportDir = useAppPersistedStore(s => s.setExportDir);
  const setActivePage = useUIStateStore(s => s.setActivePage);
  const setSettingsTab = useUIStateStore(s => s.setSettingsTab);
  const generalSettings = useGeneralSettingsStore();
  const setActiveExportProfileId = useGeneralSettingsStore(s => s.setActiveExportProfileId);
  const { handleExport, handlePickExportDir } = useImportExport();

  const defaultMergedName = (clips[0]?.originalName || "episode") + "_merged";
  const activeExportProfile = React.useMemo(
    () => getActiveExportProfile(generalSettings.exportProfiles, generalSettings.activeExportProfileId),
    [generalSettings.exportProfiles, generalSettings.activeExportProfileId]
  );
  const exportProfileOptions = React.useMemo(
    () =>
      generalSettings.exportProfiles.map((profile) => ({
        value: profile.id,
        label: profile.name.trim() || "Untitled Profile",
        description: supportsClipMerge(profile.workflow)
          ? `${getExportProfileSummary(profile)} • ${profile.mergeEnabled ? "MERGE" : "CLIPS"}`
          : getExportProfileSummary(profile),
        icon: renderProfileIcon(profile),
      })),
    [generalSettings.exportProfiles]
  );

  const canMergeWithActiveProfile = supportsClipMerge(activeExportProfile.workflow) && activeExportProfile.mergeEnabled;
  const hasSelectedClips = selectedClips.size > 0;

  const sourceClipObj = props.sourceClipSrc ? clips.find(c => c.src === props.sourceClipSrc) : null;
  const mergedSrcs = sourceClipObj?.mergedSrcs;
  const hasSource = !!props.sourceClipSrc;

  React.useEffect(() => {
    if (showMergeNameModal) {
      requestAnimationFrame(() => {
        mergeNameInputRef.current?.focus();
        mergeNameInputRef.current?.select();
      });
    }
  }, [showMergeNameModal]);

  React.useEffect(() => {
    // Force re-render when transcription changes
    setKey(prev => prev + 1);
  }, [transcription]);

  React.useEffect(() => {
    console.log('🖱️ sourceClipSrc changed to:', props.sourceClipSrc);
    console.log('🖱️ sourceClipId changed to:', props.sourceClipId);
  }, [props.sourceClipSrc, props.sourceClipId]);

  React.useEffect(() => {
    // When ANY clip's transcription updates, check if it's our selected clip
    const updatedClip = clips.find(c => c.id === props.sourceClipId);
    if (updatedClip && updatedClip.transcription !== selectedClip?.transcription) {
      console.log('🔄 Transcription updated for selected clip, forcing re-render');
      setSelectedClip(updatedClip);
      setKey(prev => prev + 1);
    }
  }, [clips, props.sourceClipId, selectedClip?.transcription]);

  React.useEffect(() => {
  if (props.sourceClipId && clips.length > 0) {
    const clip = clips.find(c => c.id === props.sourceClipId);
    console.log(`🎬 PreviewContainer - Looking for clip ID: ${props.sourceClipId}`);
    console.log(`🎬 Found clip: ${clip?.id}, transcription: ${clip?.transcription || 'none'}`);
    setSelectedClip(clip || null);
  }
  }, [props.sourceClipId, clips]);

  const onExportClick = () => {
    if (!hasSelectedClips) return;
    const targetClips = selectedClips;
    if (canMergeWithActiveProfile) {
      setShowMergeNameModal(true);
    } else {
      handleExport(targetClips, false);
    }
  };

  const confirmMergeExport = () => {
    const targetClips = selectedClips;
    const value = (mergeNameInputRef.current?.value ?? "").trim();
    if (!value) return;
    setShowMergeNameModal(false);
    handleExport(targetClips, true, value);
  };

  return (
    <main className="preview-container" >
      <div className="preview-windows-layout single">
        {hasSource && (
          <div className="preview-window-wrapper source" key="source-wrapper">
            <div className="preview-window">
              <VideoPlayer
                key={`source-player-${props.sourceClipSrc}-${key}`}
                selectedClip={props.sourceClipSrc!}
                mergedSrcs={mergedSrcs}
                videoIsHEVC={videoIsHEVC}
                userHasHEVC={userHasHEVC}
                posterPath={props.sourceClipThumbnail}
                importToken={importToken}
                onTimeUpdate={props.onTimeUpdate}
              />
            </div>
          </div>
        )}

        {!hasSource && (
          <div className="preview-window empty" key="empty-preview">
            <p>No clip selected</p>
          </div>
        )}
      </div>
      


      {hasSource && clipsLoaded && transcription && (
        <div className="transcription-box">
          <h4>Transcription</h4>
          <p>{transcription}</p>
        </div>
      )}
      {hasSource && clipsLoaded && !transcription && transcriptionEnabled && (
        <div className="transcription-box loading">
          <p>Transcribing...</p>
        </div>
      )}



      <div className="export-panel">
        <div className="export-header">
          <FaFileExport className="header-icon" />
          <span className="export-title">EXPORT SETTINGS</span>
        </div>

        <div className="export-settings-row">
          <div className="export-setting-group export-profile-group">
            <label className="export-label">
            </label>
            <div className="export-dir-row">
              <Dropdown
                className="export-profile-select"
                options={exportProfileOptions}
                value={activeExportProfile.id}
                onChange={setActiveExportProfileId}
                preferredDirection="down"
              />
              <button
                className="buttons export-dir-browse"
                onClick={() => { setSettingsTab("export"); setActivePage("settings"); }}
                title="Edit export settings"
              >
                <FaPencilAlt />
              </button>
            </div>
          </div>
        </div>

        <div className="export-path-section">
          <label className="export-label">
          </label>
          <div className="export-dir-row">
            <input
              type="text"
              className="export-dir-input"
              placeholder="Select destination..."
              value={exportDir || ""}
              onChange={(e) => setExportDir(e.target.value)}
            />
            <button
              className="buttons export-dir-browse"
              onClick={handlePickExportDir}
              title="Browse for output folder"
            >
              <FaFolderOpen />
            </button>
          </div>
        </div>

        <button
          className="buttons export-main-button"
          disabled={!hasSelectedClips}
          onClick={onExportClick}
          title={!hasSelectedClips ? "Select at least one clip to export" : "Export selected clips"}
        >
          Export Now
        </button>
      </div>

      <HowToUse />

      {showMergeNameModal && (
        <div
          className="episode-modal-overlay"
          onMouseDown={() => setShowMergeNameModal(false)}
        >
          <div
            className="episode-modal"
            onMouseDown={(e) => e.stopPropagation()}
          >
            <div className="episode-modal-title">Merged file name</div>
            <input
              ref={mergeNameInputRef}
              className="episode-modal-input"
              placeholder="Enter file name..."
              defaultValue={defaultMergedName}
              onKeyDown={(e) => {
                if (e.key === "Escape") setShowMergeNameModal(false);
                if (e.key === "Enter") confirmMergeExport();
              }}
            />
            <div className="episode-modal-actions">
              <button
                type="button"
                className="episode-modal-btn"
                onClick={() => setShowMergeNameModal(false)}
              >
                Cancel
              </button>
              <button
                type="button"
                className="episode-modal-btn primary"
                onClick={confirmMergeExport}
              >
                Export
              </button>
            </div>
          </div>
        </div>
      )}
    </main>
  );
}
