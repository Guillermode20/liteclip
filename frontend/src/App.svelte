<script lang="ts">
    import { onDestroy, onMount } from 'svelte';
    
    // Components
    import UploadArea from './components/UploadArea.svelte';
    import ProgressCard from './components/ProgressCard.svelte';
    import StatusCard from './components/StatusCard.svelte';
    import OutputPanel from './components/OutputPanel.svelte';
    import Sidebar from './components/sidebar/Sidebar.svelte';
    import Header from './components/Header.svelte';
    import FfmpegOverlay from './components/FfmpegOverlay.svelte';
    // SettingsModal is lazy-loaded below to reduce initial bundle size
    
    // Utils & Constants
    import { codecDetails, createDefaultOutputMetadata, FALLBACK_SETTINGS } from './lib/constants';
    import { formatFileSize, formatDurationLabel, formatTimeRemaining } from './utils/format';
    import { calculateOptimalResolution, getEffectiveDuration, getEffectiveMaxSize } from './utils/video';
    import { getForcedScalePercent } from './utils/resolution';
    import { getSettings, saveSettings, uploadVideo, getJobStatus, cancelJob, retryJob, getAppVersion } from './services/api';
    
    // Types
    import type {
        CodecKey,
        CompressionStatusResponse,
        OutputMetadata,
        ResolutionPreset,
        StatusMessageType,
        UpdateInfoPayload,
        UserSettingsPayload,
        VideoSegment
    } from './types';

    // ============================================================================
    // State: File & Video Source
    // ============================================================================
    let selectedFile: File | null = null;
    let sourceVideoWidth: number | null = null;
    let sourceVideoHeight: number | null = null;
    let sourceDuration: number | null = null;
    let originalSizeMb: number | null = null;
    let videoSegments: VideoSegment[] = [];
    let videoCrop: { x: number; y: number; width: number; height: number } | null = null;

    // ============================================================================
    // State: Job & Processing
    // ============================================================================
    let jobId: string | null = null;
    let fileSequenceId: number = 0;
    let statusCheckInterval: number | null = null;
    let uploadAbortController: AbortController | null = null;
    let previewAbortController: AbortController | null = null;
    let isCompressing = false;
    let progressPercent = 0;
    let showCancelButton = false;
    let canRetry = false;
    let retrying = false;
    let compressionSkipped = false;

    // ============================================================================
    // State: UI Visibility
    // ============================================================================
    let controlsVisible = false;
    let metadataVisible = false;
    let metadataContent = '';
    let statusVisible = false;
    let progressVisible = false;
    let videoPreviewVisible = false;
    let downloadVisible = false;
    let showVideoEditor = false;

    // ============================================================================
    // State: Status & Messages
    // ============================================================================
    let statusMessage = '';
    let statusType: StatusMessageType = 'processing';
    let fileInfo = '';

    // ============================================================================
    // State: Output & Download
    // ============================================================================
    let objectUrl: string | null = null;
    let videoPreviewUrl: string | null = null;
    let downloadFileName: string | null = null;
    let downloadMimeType: string | null = null;
    let outputMetadata: OutputMetadata = createDefaultOutputMetadata();

    // ============================================================================
    // State: Compression Settings
    // ============================================================================
    let outputSizeSliderValue = 100;
    let outputSizeSliderDisabled = true;
    let outputSizeValue = '--';
    let outputSizeDetails = '';
    let codecSelectValue: CodecKey = 'quality';
    let codecHelperText = codecDetails.quality.helper;
    let muteAudio = false;
    let resolutionPreset: ResolutionPreset = 'auto';
    let uploadBtnDisabled = true;
    let uploadBtnText = 'Process Video';

    // ============================================================================
    // State: User Settings & Updates
    // ============================================================================
    let userSettings: UserSettingsPayload | null = null;
    let defaultTargetMb = 25;
    let showSettingsModal = false;
    let autoUpdateEnabled = true;
    let hasCheckedUpdates = false;
    let updateInfo: UpdateInfoPayload | null = null;
    let showUpdateBanner = false;
    let appVersion: string | null = null;

    // ============================================================================
    // State: Video Editor (lazy loaded)
    // ============================================================================
    type VideoEditorComponentCtor = typeof import('./VideoEditor.svelte').default;
    let VideoEditorComponent: VideoEditorComponentCtor | null = null;
    let videoEditorModulePromise: Promise<void> | null = null;

    // ============================================================================
    // State: Settings Modal (lazy loaded)
    // ============================================================================
    type SettingsModalComponentCtor = typeof import('./components/SettingsModal.svelte').default;
    let SettingsModalComponent: SettingsModalComponentCtor | null = null;
    let settingsModalModulePromise: Promise<void> | null = null;

    // ============================================================================
    // Constants
    // ============================================================================
    const STATUS_POLL_INTERVAL_MS = 500;

    // ============================================================================
    // Derived Values
    // ============================================================================
    $: sliderConfig = (() => {
        const effectiveMax = getEffectiveMaxSize(originalSizeMb, sourceDuration, videoSegments) || (originalSizeMb || 0);
        return {
            effectiveMaxSizeNumeric: effectiveMax,
            sliderMaxRounded: Math.max(1, Math.round(effectiveMax)),
            sliderStepValue: effectiveMax < 10 ? 0.1 : 1
        };
    })();

    $: outputLabels = (() => {
        const resPct = sourceVideoWidth && sourceVideoHeight && outputMetadata.finalWidth > 0
            ? Math.round((outputMetadata.finalWidth / sourceVideoWidth) * 100)
            : null;
        
        return {
            finalBitrateLabel: outputMetadata.videoBitrateKbps > 0 
                ? `${Math.round(outputMetadata.videoBitrateKbps)} kbps` : '--',
            finalDurationLabel: formatDurationLabel(outputMetadata.finalDuration),
            finalResolutionLabel: outputMetadata.finalWidth > 0 && outputMetadata.finalHeight > 0
                ? `${outputMetadata.finalWidth}×${outputMetadata.finalHeight}${resPct ? ` (${resPct}%)` : ''}`
                : '--',
            encodingTimeLabel: outputMetadata.encodingTime > 0 
                ? formatTimeRemaining(outputMetadata.encodingTime) : '--'
        };
    })();

    // ============================================================================
    // Lifecycle
    // ============================================================================
    onMount(() => {
        loadUserSettings();
        scheduleVideoEditorPrefetch();
        fetchAppVersion();
    });

    onDestroy(() => {
        stopStatusPolling();
        cleanupVideoUrls();
    });

    // ============================================================================
    // Video Editor Prefetching
    // ============================================================================
    function scheduleVideoEditorPrefetch() {
        if (typeof window === 'undefined') return;
        const idle = (window as any).requestIdleCallback;
        if (typeof idle === 'function') {
            idle(() => prefetchVideoEditor());
        } else {
            window.setTimeout(prefetchVideoEditor, 300);
        }
    }

    function prefetchVideoEditor(): Promise<void> | null {
        if (videoEditorModulePromise) return videoEditorModulePromise;

        videoEditorModulePromise = import('./VideoEditor.svelte')
            .then((module) => { VideoEditorComponent = module.default; })
            .catch((error) => {
                if (import.meta.env.DEV) console.warn('VideoEditor preload failed', error);
                videoEditorModulePromise = null;
            });

        return videoEditorModulePromise;
    }

    // ============================================================================
    // Settings Modal Lazy Loading
    // ============================================================================
    function loadSettingsModal(): Promise<void> {
        if (settingsModalModulePromise) return settingsModalModulePromise;

        settingsModalModulePromise = import('./components/SettingsModal.svelte')
            .then((module) => { SettingsModalComponent = module.default; })
            .catch((error) => {
                if (import.meta.env.DEV) console.warn('SettingsModal load failed', error);
                settingsModalModulePromise = null;
            });

        return settingsModalModulePromise;
    }
    
    async function openSettingsModal() {
        await loadSettingsModal();
        showSettingsModal = true;
    }

    // ============================================================================
    // URL & Resource Cleanup
    // ============================================================================
    function cleanupVideoUrls() {
        if (objectUrl) { URL.revokeObjectURL(objectUrl); objectUrl = null; }
        if (videoPreviewUrl) { URL.revokeObjectURL(videoPreviewUrl); videoPreviewUrl = null; }
    }
    
    function cancelPendingOperations() {
        if (uploadAbortController) { uploadAbortController.abort(); uploadAbortController = null; }
        if (previewAbortController) { previewAbortController.abort(); previewAbortController = null; }
    }

    // ============================================================================
    // Status Polling
    // ============================================================================
    function stopStatusPolling() {
        if (statusCheckInterval) { clearInterval(statusCheckInterval); statusCheckInterval = null; }
    }

    function startStatusPolling(sequenceId: number) {
        if (typeof window === 'undefined') return;
        stopStatusPolling();
        statusCheckInterval = window.setInterval(() => checkStatus(sequenceId), STATUS_POLL_INTERVAL_MS);
    }

    function isStaleSequence(sequenceId: number): boolean {
        return sequenceId !== fileSequenceId;
    }

    // ============================================================================
    // Helper Functions
    // ============================================================================
    function getSafeEffectiveMaxSize(): number {
        const effectiveMax = getEffectiveMaxSize(originalSizeMb, sourceDuration, videoSegments);
        return effectiveMax && effectiveMax > 0 ? effectiveMax : originalSizeMb || 0;
    }

    function clampTargetSize(value: number): number {
        const maxSize = getSafeEffectiveMaxSize();
        return (!maxSize || maxSize <= 0) ? value : Math.min(value, maxSize);
    }

    function getLocalForcedScalePercent(): number | null {
        return getForcedScalePercent(sourceVideoHeight, resolutionPreset);
    }

    function showStatus(message: string, type: StatusMessageType) {
        statusMessage = message;
        statusType = type;
        statusVisible = true;
    }

    function updateCodecHelper() {
        codecHelperText = codecDetails[codecSelectValue]?.helper ?? '';
    }

    // ============================================================================
    // Job Management
    // ============================================================================
    async function cancelActiveJob() {
        if (jobId && isCompressing) {
            try { await cancelJob(jobId); } catch (e) { /* ignore */ }
        }
        stopStatusPolling();
        cancelPendingOperations();
        jobId = null;
        isCompressing = false;
        showCancelButton = false;
        progressVisible = false;
        progressPercent = 0;
    }

    async function handleCancelJob() {
        if (!jobId) return;
        if (!confirm('Are you sure you want to cancel this compression job?')) return;

        try {
            await cancelJob(jobId);
            showStatus('Cancelling compression...', 'processing');
        } catch (error) {
            console.error('Cancel failed:', error);
            showStatus('Failed to cancel processing', 'error');
        }
    }

    async function handleRetryJob() {
        if (!jobId || retrying) return;

        retrying = true;
        canRetry = false;
        isCompressing = true;
        progressVisible = true;
        progressPercent = 5;
        showCancelButton = true;
        showStatus('Re-queueing job...', 'processing');

        try {
            await retryJob(jobId);
            startStatusPolling(fileSequenceId);
        } catch (error) {
            console.error('Retry failed:', error);
            canRetry = true;
            isCompressing = false;
            progressVisible = false;
            showCancelButton = false;
            showStatus('Retry failed: ' + (error as Error).message, 'error');
        } finally {
            retrying = false;
        }
    }

    // ============================================================================
    // File Selection & Metadata
    // ============================================================================
    async function handleFileSelect(file: File) {
        if (!file.type.startsWith('video/')) {
            alert('Please select a video file');
            return;
        }

        const currentSequenceId = ++fileSequenceId;
        await cancelActiveJob();
        cleanupVideoUrls();

        // Reset state
        videoPreviewVisible = false;
        downloadVisible = false;
        downloadFileName = null;
        downloadMimeType = null;
        outputMetadata = createDefaultOutputMetadata();
        compressionSkipped = false;
        canRetry = false;
        statusVisible = false;
        videoSegments = [];
          videoCrop = null;
        selectedFile = file;
        originalSizeMb = file.size / (1024 * 1024);
        fileInfo = `Selected: ${file.name} (${formatFileSize(file.size)})`;
        uploadBtnDisabled = false;
        uploadBtnText = 'Process Video';
        controlsVisible = true;
        metadataVisible = false;
        showVideoEditor = true;
        prefetchVideoEditor();

        outputSizeSliderDisabled = true;
        outputSizeValue = '--';
        outputSizeDetails = 'Reading video metadata...';
        updateCodecHelper();
        
        if (currentSequenceId !== fileSequenceId) {
            if (import.meta.env.DEV) console.log('File selection superseded by newer operation');
        }
    }

    function handleSourceMetadataLoaded(payload: { width: number; height: number; duration: number }) {
        if (!selectedFile) return;

        sourceVideoWidth = payload.width || null;
        sourceVideoHeight = payload.height || null;
        const duration = Number.isFinite(payload.duration) ? payload.duration : null;
        sourceDuration = duration;
        
        const kbps = duration ? Math.round((selectedFile.size * 8) / duration / 1000) : null;
        const dimsText = sourceVideoWidth && sourceVideoHeight ? `${sourceVideoWidth}×${sourceVideoHeight}` : 'Unknown';
        const durationText = duration ? `${duration.toFixed(2)}s` : 'Unknown';
        const bitrateText = kbps ? `${kbps} kbps (approx)` : 'Unknown';
        
        metadataContent = `
            <div><strong>file_size</strong>: ${formatFileSize(selectedFile.size)}</div>
            <div><strong>type</strong>: ${selectedFile.type || 'unknown'}</div>
            <div><strong>duration</strong>: ${durationText}</div>
            <div><strong>resolution</strong>: ${dimsText}</div>
            <div><strong>bitrate</strong>: ${bitrateText}</div>
        `;
        metadataVisible = true;

        const safeOriginalMb = originalSizeMb || 0;
        const initialMb = Math.min(safeOriginalMb, defaultTargetMb);
        outputSizeSliderValue = initialMb > 0 ? initialMb : defaultTargetMb;
        outputSizeSliderDisabled = false;
        updateOutputSizeDisplay();
    }

    // ============================================================================
    // Settings Handlers
    // ============================================================================
    function handleSegmentsChange(segments: VideoSegment[]) {
        videoSegments = segments;
        updateOutputSizeDisplay();
    }

    function handleCropChange(crop: { x: number; y: number; width: number; height: number } | null) {
        videoCrop = crop;
    }

    function handleSliderChange(value: number) {
        if (outputSizeSliderDisabled) return;
        outputSizeSliderValue = clampTargetSize(value);
        updateOutputSizeDisplay();
    }

    function handleCodecChange(value: string) {
        codecSelectValue = value as CodecKey;
        updateCodecHelper();
        updateOutputSizeDisplay();
    }

    function handleResolutionChange(value: string) {
        resolutionPreset = value as ResolutionPreset;
        updateOutputSizeDisplay();
    }

    function handleMuteToggle(value: boolean) {
        muteAudio = value;
    }

    function handlePresetClick(targetPercent: string) {
        if (outputSizeSliderDisabled) return;
        const maxSize = getSafeEffectiveMaxSize();
        if (!maxSize || maxSize <= 0) return;
        const percent = parseFloat(targetPercent);
        outputSizeSliderValue = clampTargetSize((maxSize * percent) / 100);
        updateOutputSizeDisplay();
    }

    // ============================================================================
    // Output Size Display
    // ============================================================================
    function updateOutputSizeDisplay() {
        if (!originalSizeMb || !Number.isFinite(originalSizeMb)) {
            outputSizeValue = '--';
            outputSizeDetails = '';
            return;
        }

        let targetSizeMb = parseFloat(outputSizeSliderValue.toString());
        const effectiveMaxSize = getSafeEffectiveMaxSize();
        
        if (effectiveMaxSize > 0 && targetSizeMb > effectiveMaxSize) {
            targetSizeMb = effectiveMaxSize;
            outputSizeSliderValue = effectiveMaxSize;
        }
        
        const displayValue = targetSizeMb >= 10 ? targetSizeMb.toFixed(0) : targetSizeMb.toFixed(1);
        outputSizeValue = `${displayValue} MB`;

        if (videoSegments.length > 0 && effectiveMaxSize !== originalSizeMb) {
            outputSizeValue += ` (max: ${Math.round(effectiveMaxSize)} MB)`;
        }

        if (!sourceDuration || !sourceVideoWidth || !sourceVideoHeight) {
            outputSizeDetails = 'Waiting for video metadata...';
            return;
        }

        const effectiveDuration = getEffectiveDuration(videoSegments, sourceDuration) ?? sourceDuration;

        if (targetSizeMb >= effectiveMaxSize) {
            outputSizeDetails = videoSegments.length > 0 && effectiveDuration !== sourceDuration
                ? 'Will cut video segments only (no compression)'
                : 'No compression (original quality preserved)';
            return;
        }

        const targetBitsTotal = targetSizeMb * 1024 * 1024 * 8 * 0.9;
        const targetBitrateKbps = targetBitsTotal / effectiveDuration / 1000;
        const forcedScale = getLocalForcedScalePercent();
        const recommendedScale = forcedScale ?? calculateOptimalResolution(targetSizeMb, effectiveDuration, sourceVideoWidth, sourceVideoHeight);
        const appliedScale = Math.max(10, Math.min(100, recommendedScale));
        const targetW = Math.floor(((sourceVideoWidth * appliedScale) / 100) / 2) * 2;
        const targetH = Math.floor(((sourceVideoHeight * appliedScale) / 100) / 2) * 2;

        let details = `Target bitrate: ~${Math.round(targetBitrateKbps)} kbps`;
        details += appliedScale < 100 
            ? ` · Resolution: ${targetW}×${targetH} (${appliedScale}%)`
            : ` · Resolution: ${sourceVideoWidth}×${sourceVideoHeight} (original)`;

        if (videoSegments.length > 0 && effectiveDuration !== sourceDuration) {
            details += ` · Duration: ${effectiveDuration.toFixed(1)}s (edited)`;
        }

        outputSizeDetails = details;
    }

    // ============================================================================
    // Upload & Compression
    // ============================================================================
    async function handleUpload(event: MouseEvent) {
        event.stopPropagation();
        if (!selectedFile || !sourceDuration || !sourceVideoWidth || !sourceVideoHeight) {
            showStatus('Video metadata missing. Please re-select the file.', 'error');
            return;
        }

        if (uploadAbortController) uploadAbortController.abort();
        uploadAbortController = new AbortController();
        const currentSequenceId = fileSequenceId;

        uploadBtnDisabled = true;
        uploadBtnText = 'Uploading...';
        progressVisible = true;
        progressPercent = 10;
        canRetry = false;

        const formData = new FormData();
        formData.append('file', selectedFile);
        formData.append('codec', codecSelectValue);

        const targetSizeMb = parseFloat(outputSizeSliderValue.toString());
        const forcedScalePercent = getLocalForcedScalePercent();
        const shouldForceResolution = forcedScalePercent !== null;
        const effectiveMaxSize = getSafeEffectiveMaxSize();
        const effectiveDuration = getEffectiveDuration(videoSegments, sourceDuration) ?? sourceDuration;
        
        formData.append('targetSizeMb', targetSizeMb.toFixed(2));
        formData.append('skipCompression', (targetSizeMb >= effectiveMaxSize && !shouldForceResolution && !muteAudio) ? 'true' : 'false');
        formData.append('qualityMode', codecSelectValue === 'quality' ? 'true' : 'false');
        formData.append('muteAudio', muteAudio ? 'true' : 'false');

        if (shouldForceResolution && forcedScalePercent !== null) {
            formData.append('scalePercent', forcedScalePercent.toString());
        } else if (targetSizeMb < effectiveMaxSize) {
            const calculatedScale = calculateOptimalResolution(targetSizeMb, effectiveDuration, sourceVideoWidth, sourceVideoHeight);
            if (Number.isFinite(calculatedScale)) formData.append('scalePercent', calculatedScale.toString());
        } else {
            formData.append('scalePercent', '100');
        }

        formData.append('sourceDuration', sourceDuration.toFixed(3));
        formData.append('sourceWidth', sourceVideoWidth.toString());
        formData.append('sourceHeight', sourceVideoHeight.toString());
        formData.append('originalSizeBytes', selectedFile.size.toString());

        if (videoSegments.length > 0) {
            formData.append('segments', JSON.stringify(videoSegments));
        }

        if (videoCrop && sourceVideoWidth && sourceVideoHeight) {
            const cropX = Math.round((videoCrop.x / 100) * sourceVideoWidth);
            const cropY = Math.round((videoCrop.y / 100) * sourceVideoHeight);
            const cropW = Math.round((videoCrop.width / 100) * sourceVideoWidth);
            const cropH = Math.round((videoCrop.height / 100) * sourceVideoHeight);
            
            formData.append('cropX', cropX.toString());
            formData.append('cropY', cropY.toString());
            formData.append('cropWidth', cropW.toString());
            formData.append('cropHeight', cropH.toString());
        }

        try {
            const result = await uploadVideo(formData, uploadAbortController.signal);
            if (currentSequenceId !== fileSequenceId) return;
            
            jobId = result.jobId;
            progressPercent = 100;
            isCompressing = true;
            showStatus('Video uploaded successfully. Processing...', 'processing');
            startStatusPolling(currentSequenceId);
        } catch (error) {
            if (error instanceof Error && error.name === 'AbortError') return;
            
            console.error('Upload failed:', error);
            let errorMessage = (error as Error).message;
            if (errorMessage.includes('NetworkError') || errorMessage.includes('Failed to fetch')) {
                errorMessage = 'Network error: File may be too large or server is unreachable.';
            } else if (errorMessage.includes('413')) {
                errorMessage = 'File is too large. The server cannot accept files this big.';
            }

            showStatus('Upload failed: ' + errorMessage, 'error');
            uploadBtnDisabled = false;
            uploadBtnText = 'Process Video';
            progressVisible = false;
        } finally {
            uploadAbortController = null;
        }
    }

    // ============================================================================
    // Status Checking
    // ============================================================================
    async function checkStatus(sequenceId: number) {
        if (!jobId || isStaleSequence(sequenceId)) {
            stopStatusPolling();
            return;
        }
        
        try {
            const result = await getJobStatus(jobId);
            if (isStaleSequence(sequenceId)) return;
            
            if (result.status === 'queued') {
                showCancelButton = true;
                canRetry = false;
                const queueMsg = result.queuePosition && result.queuePosition > 0
                    ? `Queued for processing (position ${result.queuePosition})...`
                    : 'Queued for processing...';
                showStatus(queueMsg, 'processing');
            } else if (result.status === 'processing') {
                showCancelButton = true;
                canRetry = false;
                const progressPercentValue = Math.max(10, Math.min(95, result.progress || 0));
                progressPercent = progressPercentValue;

                const statusText = result.estimatedSecondsRemaining && result.estimatedSecondsRemaining > 0
                    ? `Processing video... ${progressPercentValue.toFixed(1)}% (ETA: ${formatTimeRemaining(result.estimatedSecondsRemaining)})`
                    : `Processing video... ${progressPercentValue.toFixed(1)}%`;
                showStatus(statusText, 'processing');

                outputMetadata = {
                    ...outputMetadata,
                    codec: result.codec || outputMetadata.codec,
                    encoderName: result.encoderName ?? outputMetadata.encoderName,
                    encoderIsHardware: result.encoderIsHardware ?? outputMetadata.encoderIsHardware
                };
            } else if (result.status === 'completed') {
                stopStatusPolling();
                if (isStaleSequence(sequenceId)) return;
                
                progressPercent = 100;
                isCompressing = false;
                showCancelButton = false;
                downloadFileName = result.outputFilename || `compressed_${selectedFile?.name ?? jobId}`;
                downloadMimeType = result.outputMimeType || 'video/mp4';

                calculateOutputMetadata(result);
                await loadVideoPreview(sequenceId);

                showStatus('Processing complete! Preview and download your video.', 'success');
                videoPreviewVisible = true;
                downloadVisible = true;
                progressVisible = false;
                canRetry = false;
            } else if (result.status === 'cancelled') {
                stopStatusPolling();
                isCompressing = false;
                showCancelButton = false;
                showStatus('Processing was cancelled.', 'error');
                uploadBtnDisabled = false;
                uploadBtnText = 'Process Video';
                progressVisible = false;
                canRetry = false;
            } else if (result.status === 'failed') {
                stopStatusPolling();
                isCompressing = false;
                showCancelButton = false;
                showStatus('Processing failed: ' + (result.message || 'Unknown error'), 'error');
                uploadBtnDisabled = false;
                uploadBtnText = 'Process Video';
                progressVisible = false;
                canRetry = true;
            }
        } catch (error) {
            console.error('Status check failed:', error);
            stopStatusPolling();
            isCompressing = false;
            showStatus('Failed to check status: ' + (error as Error).message, 'error');
            uploadBtnDisabled = false;
            uploadBtnText = 'Process Video';
            progressVisible = false;
        }
    }

    // ============================================================================
    // Output Metadata Calculation
    // ============================================================================
    function calculateOutputMetadata(result: CompressionStatusResponse) {
        if (!originalSizeMb || !sourceDuration) return;

        const effectiveDuration = getEffectiveDuration(videoSegments, sourceDuration) ?? sourceDuration;
        const effectiveMaxSize = getEffectiveMaxSize(originalSizeMb, sourceDuration, videoSegments);

        const actualOutputBytes = typeof result.outputSizeBytes === 'number' && Number.isFinite(result.outputSizeBytes) && result.outputSizeBytes > 0
            ? result.outputSizeBytes : null;

        let estimatedOutputBytes = 0;
        let outputSizeMb = 0;

        if (actualOutputBytes) {
            estimatedOutputBytes = actualOutputBytes;
            outputSizeMb = actualOutputBytes / (1024 * 1024);
        } else if (result.targetBitrateKbps && result.targetBitrateKbps > 0) {
            estimatedOutputBytes = Math.round((result.targetBitrateKbps * 1000 * effectiveDuration) / 8);
            outputSizeMb = estimatedOutputBytes / (1024 * 1024);
        } else if (result.videoBitrateKbps && result.videoBitrateKbps > 0) {
            const totalBitrateKbps = result.videoBitrateKbps + 128;
            estimatedOutputBytes = Math.round((totalBitrateKbps * 1000 * effectiveDuration) / 8);
            outputSizeMb = estimatedOutputBytes / (1024 * 1024);
        } else {
            outputSizeMb = effectiveMaxSize;
            estimatedOutputBytes = Math.round(effectiveMaxSize * 1024 * 1024);
        }

        compressionSkipped = result.compressionSkipped === true;

        const ratioSizeMb = actualOutputBytes ? actualOutputBytes / (1024 * 1024) : outputSizeMb;
        const safeEffectiveMaxSize = effectiveMaxSize > 0 ? effectiveMaxSize : ratioSizeMb || 1;
        const compressionRatio = compressionSkipped ? 0 : (1 - ratioSizeMb / safeEffectiveMaxSize) * 100;
        const startTime = new Date(result.createdAt || Date.now());
        const completionTime = new Date(result.completedAt || Date.now());
        const encodingSeconds = Math.max(0, (completionTime.getTime() - startTime.getTime()) / 1000);

        outputMetadata = {
            outputSizeBytes: estimatedOutputBytes,
            outputSizeMb,
            compressionRatio,
            targetBitrateKbps: result.targetBitrateKbps || 0,
            videoBitrateKbps: result.videoBitrateKbps || 0,
            estimatedVideoBitrateKbps: result.videoBitrateKbps || 0,
            scalePercent: result.scalePercent || 100,
            codec: result.codec || 'h264',
            encoderName: result.encoderName || null,
            encoderIsHardware: result.encoderIsHardware ?? false,
            encodingTime: Math.round(encodingSeconds),
            finalDuration: 0,
            finalWidth: 0,
            finalHeight: 0
        };
    }

    // ============================================================================
    // Video Preview Loading
    // ============================================================================
    async function loadVideoPreview(sequenceId: number) {
        if (!jobId || sequenceId !== fileSequenceId) return;

        if (previewAbortController) previewAbortController.abort();
        previewAbortController = new AbortController();

        try {
            const response = await fetch(`/api/download/${jobId}`, { signal: previewAbortController.signal });
            if (sequenceId !== fileSequenceId) return;
            
            if (response.ok) {
                const blob = await response.blob();
                if (sequenceId !== fileSequenceId) return;
                
                if (videoPreviewUrl) URL.revokeObjectURL(videoPreviewUrl);
                videoPreviewUrl = URL.createObjectURL(blob);

                const actualSizeMb = blob.size / (1024 * 1024);
                const effectiveMaxSize = getEffectiveMaxSize(originalSizeMb, sourceDuration, videoSegments);
                const compressionRatio = compressionSkipped ? 0 : effectiveMaxSize > 0 ? (1 - actualSizeMb / effectiveMaxSize) * 100 : 0;
                
                outputMetadata = {
                    ...outputMetadata,
                    outputSizeBytes: blob.size,
                    outputSizeMb: actualSizeMb,
                    compressionRatio,
                    finalDuration: 0,
                    finalWidth: 0,
                    finalHeight: 0
                };
            }
        } catch (error) {
            if (error instanceof Error && error.name === 'AbortError') return;
            console.warn('Failed to load video preview:', error);
        } finally {
            previewAbortController = null;
        }
    }

    // ============================================================================
    // Result Handlers
    // ============================================================================
    function handleClearResult() {
        cancelPendingOperations();
        if (videoPreviewUrl) { URL.revokeObjectURL(videoPreviewUrl); videoPreviewUrl = null; }

        jobId = null;
        downloadFileName = null;
        downloadMimeType = null;
        videoPreviewVisible = false;
        downloadVisible = false;
        statusVisible = false;
        progressVisible = false;
        progressPercent = 0;
        isCompressing = false;
        showCancelButton = false;
        canRetry = false;
        outputMetadata = createDefaultOutputMetadata();
        compressionSkipped = false;
        uploadBtnDisabled = false;
        uploadBtnText = 'Process Video';

        showStatus('Result cleared. You can adjust settings and compress again.', 'success');
        setTimeout(() => { statusVisible = false; }, 3000);
    }

    function handleCompressedMetadata(event: CustomEvent<{ duration: number | null; width: number; height: number }>) {
        const { duration, width, height } = event.detail;
        const sizeBytes = outputMetadata.outputSizeBytes;
        const bitrateKbps = duration && sizeBytes ? Math.round((sizeBytes * 8) / duration / 1000) : outputMetadata.videoBitrateKbps;

        outputMetadata = {
            ...outputMetadata,
            videoBitrateKbps: bitrateKbps,
            estimatedVideoBitrateKbps: bitrateKbps,
            finalDuration: duration ?? outputMetadata.finalDuration,
            finalWidth: width,
            finalHeight: height
        };
    }

    function handleDownload() {
        if (!jobId) return;
        const link = document.createElement('a');
        link.href = `/api/download/${jobId}`;
        link.download = downloadFileName || `compressed_${selectedFile?.name ?? jobId}`;
        if (downloadMimeType) link.type = downloadMimeType;
        document.body.appendChild(link);
        link.click();
        document.body.removeChild(link);
        resetInterface();
    }

    // ============================================================================
    // Interface Reset
    // ============================================================================
    function resetInterface() {
        stopStatusPolling();
        selectedFile = null;
        jobId = null;
        fileInfo = '';
        statusVisible = false;
        downloadVisible = false;
        videoPreviewVisible = false;
        progressVisible = false;
        progressPercent = 0;
        isCompressing = false;
        uploadBtnDisabled = true;
        uploadBtnText = 'Process Video';
        controlsVisible = false;
        metadataVisible = false;
        downloadFileName = null;
        downloadMimeType = null;
        showCancelButton = false;
        compressionSkipped = false;
        cleanupVideoUrls();
        outputSizeSliderDisabled = true;
        outputSizeSliderValue = defaultTargetMb;
        outputSizeValue = '--';
        outputSizeDetails = '';
        codecSelectValue = userSettings?.defaultCodec ?? 'quality';
        updateCodecHelper();
        sourceVideoWidth = null;
        sourceVideoHeight = null;
        sourceDuration = null;
        originalSizeMb = null;
        muteAudio = userSettings?.defaultMuteAudio ?? false;
        resolutionPreset = userSettings?.defaultResolution ?? 'auto';
        canRetry = false;
        outputMetadata = createDefaultOutputMetadata();
        showVideoEditor = false;
        videoSegments = [];
    }

    // ============================================================================
    // User Settings
    // ============================================================================
    async function loadUserSettings() {
        let fetched: UserSettingsPayload | null = null;
        try {
            fetched = await getSettings();
        } catch (error) {
            console.warn('Settings fetch failed', error);
        } finally {
            userSettings = fetched ?? { ...FALLBACK_SETTINGS };
            applyUserSettings(userSettings);
        }
    }

    function applyUserSettings(settings: UserSettingsPayload | null) {
        const effective = settings ?? FALLBACK_SETTINGS;
        codecSelectValue = effective.defaultCodec;
        updateCodecHelper();
        resolutionPreset = effective.defaultResolution;
        muteAudio = effective.defaultMuteAudio;
        defaultTargetMb = effective.defaultTargetSizeMb;
        autoUpdateEnabled = effective.checkForUpdatesOnLaunch;

        const scale = effective.appScale ?? 1.0;
        document.documentElement.style.setProperty('--app-scale', String(scale));
        document.documentElement.style.fontSize = `${16 * scale}px`;

        if (!selectedFile) outputSizeSliderValue = defaultTargetMb;
        if (autoUpdateEnabled && !hasCheckedUpdates) checkForUpdates();
        if (!autoUpdateEnabled) showUpdateBanner = false;
    }

    async function handleSettingsSave(event: CustomEvent<UserSettingsPayload>) {
        const payload = event.detail;
        try {
            const saved = await saveSettings(payload);
            userSettings = saved;
            applyUserSettings(saved);
            showSettingsModal = false;
            showStatus('Settings saved', 'success');
            setTimeout(() => { statusVisible = false; }, 2000);
        } catch (error) {
            console.error('Save settings failed:', error);
            showStatus('Failed to save settings: ' + (error as Error).message, 'error');
        }
    }

    // ============================================================================
    // App Version & Updates
    // ============================================================================
    async function fetchAppVersion() {
        try {
            const payload = await getAppVersion();
            if (payload?.version) {
                appVersion = payload.version;
                if (updateInfo === null) {
                    updateInfo = {
                        currentVersion: payload.version,
                        latestVersion: payload.version,
                        updateAvailable: false,
                        downloadUrl: null,
                        checkedAt: undefined,
                        releaseNotes: null
                    };
                } else {
                    updateInfo = { ...updateInfo, currentVersion: payload.version };
                }
            }
        } catch (error) {
            console.warn('Failed to fetch app version', error);
        }
    }

    async function checkForUpdates() {
        if (!autoUpdateEnabled || hasCheckedUpdates) return;

        hasCheckedUpdates = true;
        try {
            const response = await fetch('/api/update');
            if (!response.ok) return;
            const payload: UpdateInfoPayload = await response.json();
            const normalizedVersion = payload.currentVersion ?? appVersion ?? '0.0.0';
            appVersion = normalizedVersion;
            updateInfo = { ...payload, currentVersion: normalizedVersion };
            showUpdateBanner = payload.updateAvailable === true;
        } catch (error) {
            console.warn('Update check failed', error);
        }
    }

    function dismissUpdateBanner() {
        showUpdateBanner = false;
    }
</script>

<div class="app-layout">
    <Header
        updateInfo={updateInfo}
        showUpdateBanner={showUpdateBanner}
        on:openSettings={openSettingsModal}
        on:dismissUpdate={dismissUpdateBanner}
    />

    <div class="main-layout">
        <main class="main-content">
            {#if !selectedFile && !videoPreviewVisible && !progressVisible}
                <UploadArea
                    selectedFile={selectedFile}
                    hasControls={controlsVisible}
                    fileInfo={fileInfo}
                    on:fileSelected={(event) => handleFileSelect(event.detail.file)}
                />
            {/if}

            {#if showVideoEditor && selectedFile && !videoPreviewVisible && !progressVisible}
                <div class="content-card">
                    {#if VideoEditorComponent}
                        <svelte:component
                            this={VideoEditorComponent}
                            videoFile={selectedFile}
                            onSegmentsChange={handleSegmentsChange}
                            onCropChange={handleCropChange}
                            onRemoveVideo={resetInterface}
                            onMetadataLoaded={handleSourceMetadataLoaded}
                        />
                    {:else}
                        <div class="video-editor-placeholder">
                            Initializing editor...
                        </div>
                    {/if}
                </div>
            {/if}

            {#if progressVisible}
                <ProgressCard {progressPercent} {isCompressing} />
            {/if}

            {#if statusVisible}
                <StatusCard message={statusMessage} type={statusType} />
                {#if statusType === 'error' && canRetry}
                    <button class="retry-btn" on:click={handleRetryJob} disabled={retrying}>
                        $ {retrying ? 'retrying...' : 'retry job'}
                    </button>
                {/if}
            {/if}

            {#if videoPreviewVisible}
                <OutputPanel
                    videoUrl={videoPreviewUrl}
                    downloadMimeType={downloadMimeType || 'video/mp4'}
                    {outputMetadata}
                    {originalSizeMb}
                    finalBitrateLabel={outputLabels.finalBitrateLabel}
                    finalResolutionLabel={outputLabels.finalResolutionLabel}
                    finalDurationLabel={outputLabels.finalDurationLabel}
                    encodingTimeLabel={outputLabels.encodingTimeLabel}
                    downloadDisabled={!downloadVisible}
                    on:metadata={handleCompressedMetadata}
                    on:download={handleDownload}
                    on:clear={handleClearResult}
                />
            {/if}
        </main>

        {#if selectedFile}
            <Sidebar
                {metadataVisible}
                {metadataContent}
                {controlsVisible}
                {outputSizeValue}
                {outputSizeDetails}
                {outputSizeSliderValue}
                {outputSizeSliderDisabled}
                sliderMax={sliderConfig.sliderMaxRounded}
                sliderStep={sliderConfig.sliderStepValue}
                codecSelectValue={codecSelectValue}
                codecHelperText={codecHelperText}
                uploadBtnDisabled={uploadBtnDisabled}
                uploadBtnText={uploadBtnText}
                {showCancelButton}
                {muteAudio}
                resolutionPreset={resolutionPreset}
                onPresetClick={handlePresetClick}
                onSliderChange={handleSliderChange}
                onCodecChange={handleCodecChange}
                onUploadClick={handleUpload}
                onCancelClick={handleCancelJob}
                onMuteToggle={handleMuteToggle}
                onResolutionChange={handleResolutionChange}
            />
        {/if}
    </div>
</div>

<FfmpegOverlay />

{#if SettingsModalComponent}
    <svelte:component
        this={SettingsModalComponent}
        open={showSettingsModal}
        settings={userSettings}
        on:close={() => (showSettingsModal = false)}
        on:save={handleSettingsSave}
    />
{/if}
