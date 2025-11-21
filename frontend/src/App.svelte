<script lang="ts">
    import { onDestroy, onMount } from 'svelte';
    import UploadArea from './components/UploadArea.svelte';
    import ProgressCard from './components/ProgressCard.svelte';
    import StatusCard from './components/StatusCard.svelte';
    import OutputPanel from './components/OutputPanel.svelte';
    import Sidebar from './components/sidebar/Sidebar.svelte';
    import SettingsModal from './components/SettingsModal.svelte';
    import VideoEditor from './VideoEditor.svelte';
    import { codecDetails, createDefaultOutputMetadata } from './lib/constants';
    import type {
        CodecKey,
        CompressionStatusResponse,
        OutputMetadata,
        ResolutionPreset,
        StatusMessageType,
        UpdateInfoPayload,
        UserSettingsPayload,
        VideoSegment,
        FfmpegStatusResponse
    } from './types';
    import { formatFileSize, formatDurationLabel, formatTimeRemaining } from './utils/format';
    import { calculateOptimalResolution, getEffectiveDuration, getEffectiveMaxSize } from './utils/video';
    import {
        getSettings,
        saveSettings,
        getFfmpegStatus,
        retryFfmpeg,
        uploadVideo,
        getJobStatus,
        cancelJob,
        retryJob
    } from './services/api';

    let selectedFile: File | null = null;
    let jobId: string | null = null;
    let statusCheckInterval: number | null = null;
    let downloadFileName: string | null = null;
    let downloadMimeType: string | null = null;
    let objectUrl: string | null = null;
    let sourceVideoWidth: number | null = null;
    let sourceVideoHeight: number | null = null;
    let sourceDuration: number | null = null;
    let originalSizeMb: number | null = null;

    let fileInfo = '';
    let metadataVisible = false;
    let metadataContent = '';
    let controlsVisible = false;
    let statusVisible = false;
    let statusMessage = '';
    let statusType: StatusMessageType = 'processing';
    let progressVisible = false;
    let progressPercent = 0;
    let isCompressing = false;
    let downloadVisible = false;
    let videoPreviewVisible = false;
    let videoPreviewUrl: string | null = null;
    let uploadBtnDisabled = true;
    let uploadBtnText = 'Process Video';
    let outputSizeSliderDisabled = true;
    let outputSizeValue = '--';
    let outputSizeDetails = '';
    let outputSizeSliderValue = 100;
    let sliderStepValue = 1;
    let effectiveMaxSizeNumeric: number = 0;
    let sliderMaxRounded: number = 100;
    let codecSelectValue: CodecKey = 'quality';
    let codecHelperText = codecDetails.quality.helper;
    let showCancelButton = false;
    let compressionSkipped = false;
    let showVideoEditor = false;
    let videoSegments: VideoSegment[] = [];
    let muteAudio = false;
    let resolutionPreset: ResolutionPreset = 'auto';
    let canRetry = false;
    let retrying = false;
    let updateInfo: UpdateInfoPayload | null = null;
    let showUpdateBanner = false;
    let userSettings: UserSettingsPayload | null = null;
    let defaultTargetMb = 25;
    let showSettingsModal = false;
    let autoUpdateEnabled = true;
    let hasCheckedUpdates = false;

    let ffmpegReady = false;
    let ffmpegStatusMessage = 'Preparing FFmpeg dependencies...';
    let ffmpegProgressPercent = 0;
    let ffmpegError: string | null = null;
    let ffmpegStatusInterval: number | null = null;
    let ffmpegState: FfmpegStatusResponse['state'] = 'idle';
    let ffmpegRetrying = false;

    const fallbackSettings: UserSettingsPayload = {
        defaultCodec: 'quality',
        defaultResolution: 'auto',
        defaultMuteAudio: false,
        defaultTargetSizeMb: 25,
        checkForUpdatesOnLaunch: true
    };

    let outputMetadata: OutputMetadata = createDefaultOutputMetadata();

    onDestroy(() => {
        if (statusCheckInterval) {
            clearInterval(statusCheckInterval);
        }
        stopFfmpegPolling();
        if (objectUrl) {
            URL.revokeObjectURL(objectUrl);
        }
        if (videoPreviewUrl) {
            URL.revokeObjectURL(videoPreviewUrl);
        }
    });

    onMount(() => {
        loadUserSettings();
        startFfmpegPolling();
    });

    // Reactive derived values for slider
    $: effectiveMaxSizeNumeric = getEffectiveMaxSize(originalSizeMb, sourceDuration, videoSegments) || (originalSizeMb || 0);
    $: sliderMaxRounded = Math.max(1, Math.round(effectiveMaxSizeNumeric));
    $: sliderStepValue = effectiveMaxSizeNumeric < 10 ? 0.1 : 1;

    function handleFileSelect(file: File) {
        if (!file.type.startsWith('video/')) {
            alert('Please select a video file');
            return;
        }

        videoSegments = [];
        selectedFile = file;
        originalSizeMb = file.size / (1024 * 1024);
        fileInfo = `Selected: ${file.name} (${formatFileSize(file.size)})`;
        uploadBtnDisabled = false;
        uploadBtnText = 'Process Video';
        controlsVisible = true;
        metadataVisible = false;
        showVideoEditor = true;

        outputSizeSliderDisabled = true;
        outputSizeValue = '--';
        outputSizeDetails = 'Reading video metadata...';

        updateCodecHelper();

        if (objectUrl) {
            URL.revokeObjectURL(objectUrl);
        }
        objectUrl = URL.createObjectURL(file);
        const videoEl = document.createElement('video');
        videoEl.preload = 'metadata';
        videoEl.src = objectUrl;
        videoEl.addEventListener(
            'loadedmetadata',
            () => {
                sourceVideoWidth = videoEl.videoWidth || null;
                sourceVideoHeight = videoEl.videoHeight || null;
                const duration = isFinite(videoEl.duration) ? videoEl.duration : null;
                sourceDuration = duration;
                const kbps = duration ? Math.round((file.size * 8) / duration / 1000) : null;
                const dimsText =
                    sourceVideoWidth && sourceVideoHeight
                        ? `${sourceVideoWidth}×${sourceVideoHeight}`
                        : 'Unknown';
                const durationText = duration ? `${duration.toFixed(2)}s` : 'Unknown';
                const bitrateText = kbps ? `${kbps} kbps (approx)` : 'Unknown';
                metadataContent = `
                    <div><strong>file_size</strong>: ${formatFileSize(file.size)}</div>
                    <div><strong>type</strong>: ${file.type || 'unknown'}</div>
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
            },
            { once: true }
        );
    }

    function updateCodecHelper() {
        codecHelperText = codecDetails[codecSelectValue]?.helper ?? '';
    }

    function handleSegmentsChange(segments: VideoSegment[]) {
        videoSegments = segments;
        updateOutputSizeDisplay();
    }

    function handleSliderChange(value: number) {
        if (outputSizeSliderDisabled) return;
        const effectiveMax = getEffectiveMaxSize(originalSizeMb, sourceDuration, videoSegments) || (originalSizeMb || 0);
        if (effectiveMax && value > effectiveMax) {
            outputSizeSliderValue = effectiveMax;
        } else {
            outputSizeSliderValue = value;
        }
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

    function parseResolutionHeight(preset: ResolutionPreset): number | null {
        switch (preset) {
            case '1080p':
                return 1080;
            case '720p':
                return 720;
            case '480p':
                return 480;
            case '360p':
                return 360;
            default:
                return null;
        }
    }

    function getForcedScalePercent(): number | null {
        if (!sourceVideoHeight || resolutionPreset === 'auto') {
            return null;
        }

        if (resolutionPreset === 'source') {
            return 100;
        }

        const targetHeight = parseResolutionHeight(resolutionPreset);
        if (!targetHeight || targetHeight <= 0) {
            return null;
        }

        if (sourceVideoHeight <= targetHeight) {
            return 100;
        }

        const percent = Math.round((targetHeight / sourceVideoHeight) * 100);
        return Math.max(10, Math.min(100, percent));
    }

    function clampPercentValue(value: number | null | undefined) {
        if (typeof value !== 'number' || !Number.isFinite(value)) {
            return 100;
        }
        return Math.min(100, Math.max(1, value));
    }

    async function loadUserSettings() {
        let fetched: UserSettingsPayload | null = null;
        try {
            fetched = await getSettings();
        } catch (error) {
            console.warn('Settings fetch failed', error);
        } finally {
            userSettings = fetched ?? { ...fallbackSettings };
            applyUserSettings(userSettings);
        }
    }

    function startFfmpegPolling() {
        if (ffmpegStatusInterval !== null) {
            return;
        }
        fetchFfmpegStatus();
        ffmpegStatusInterval = window.setInterval(fetchFfmpegStatus, 1500);
    }

    function stopFfmpegPolling() {
        if (ffmpegStatusInterval !== null) {
            clearInterval(ffmpegStatusInterval);
            ffmpegStatusInterval = null;
        }
    }

    async function fetchFfmpegStatus() {
        try {
            const payload = await getFfmpegStatus();
            ffmpegState = payload.state;
            ffmpegReady = payload.ready;
            ffmpegProgressPercent = typeof payload.progressPercent === 'number' ? payload.progressPercent : 0;
            ffmpegStatusMessage = payload.message ?? 'Preparing FFmpeg dependencies...';
            ffmpegError = payload.errorMessage ?? null;

            if (payload.ready) {
                stopFfmpegPolling();
            }
        } catch (error) {
            ffmpegError = (error as Error).message || 'Unable to check FFmpeg status';
            ffmpegStatusMessage = 'Unable to check FFmpeg status';
        }
    }

    async function handleFfmpegRetry() {
        if (ffmpegRetrying) return;
        ffmpegRetrying = true;
        ffmpegStatusMessage = 'Retrying FFmpeg download...';
        try {
            await retryFfmpeg();
            ffmpegError = null;
            stopFfmpegPolling();
            startFfmpegPolling();
        } catch (error) {
            ffmpegError = (error as Error).message;
        } finally {
            ffmpegRetrying = false;
        }
    }

    function handleLogoError(e: Event) {
        const img = e?.target as HTMLImageElement | null;
        if (img) img.src = '/assets/logo.svg';
    }

    function applyUserSettings(settings: UserSettingsPayload | null) {
        const effective = settings ?? fallbackSettings;
        codecSelectValue = effective.defaultCodec;
        updateCodecHelper();
        resolutionPreset = effective.defaultResolution;
        muteAudio = effective.defaultMuteAudio;
        defaultTargetMb = effective.defaultTargetSizeMb;
        autoUpdateEnabled = effective.checkForUpdatesOnLaunch;

        if (!selectedFile) {
            outputSizeSliderValue = defaultTargetMb;
        }

        if (autoUpdateEnabled && !hasCheckedUpdates) {
            checkForUpdates();
        }

        if (!autoUpdateEnabled) {
            showUpdateBanner = false;
        }
    }

    async function handleSettingsSave(event: CustomEvent<UserSettingsPayload>) {
        const payload = event.detail;
        try {
            const saved = await saveSettings(payload);
            userSettings = saved;
            applyUserSettings(saved);
            showSettingsModal = false;
            showStatus('Settings saved', 'success');
            setTimeout(() => {
                statusVisible = false;
            }, 2000);
        } catch (error) {
            console.error('Save settings failed:', error);
            showStatus('Failed to save settings: ' + (error as Error).message, 'error');
        }
    }

    function handlePresetClick(targetPercent: string) {
        if (outputSizeSliderDisabled) return;
        const effectiveMax = getEffectiveMaxSize(originalSizeMb, sourceDuration, videoSegments) || (originalSizeMb || 0);
        if (!effectiveMax || effectiveMax <= 0) return;
        const percent = parseFloat(targetPercent);
        outputSizeSliderValue = (effectiveMax * percent) / 100;
        updateOutputSizeDisplay();
    }

    function updateOutputSizeDisplay() {
        if (!originalSizeMb || !Number.isFinite(originalSizeMb)) {
            outputSizeValue = '--';
            outputSizeDetails = '';
            return;
        }

        let targetSizeMb = parseFloat(outputSizeSliderValue.toString());
        const effectiveMaxSize = getEffectiveMaxSize(originalSizeMb, sourceDuration, videoSegments);
        
        // Respect effective max size computed from edited segments - clamp user value
        if (effectiveMaxSize > 0 && targetSizeMb > effectiveMaxSize) {
            targetSizeMb = effectiveMaxSize;
            outputSizeSliderValue = effectiveMaxSize;
        }
        
        const displayValue = targetSizeMb >= 10 ? targetSizeMb.toFixed(0) : targetSizeMb.toFixed(1);
        outputSizeValue = `${displayValue} MB`;

        if (videoSegments.length > 0 && effectiveMaxSize !== originalSizeMb) {
            // Round to nearest whole megabyte for display
            outputSizeValue += ` (max: ${Math.round(effectiveMaxSize)} MB)`;
        }

        if (!sourceDuration || !sourceVideoWidth || !sourceVideoHeight) {
            outputSizeDetails = 'Waiting for video metadata...';
            return;
        }

        const effectiveDuration = getEffectiveDuration(videoSegments, sourceDuration) ?? sourceDuration;

        if (targetSizeMb >= effectiveMaxSize) {
            outputSizeDetails =
                videoSegments.length > 0 && effectiveDuration !== sourceDuration
                    ? 'Will cut video segments only (no compression)'
                    : 'No compression (original quality preserved)';
            return;
        }

        const targetBitsTotal = targetSizeMb * 1024 * 1024 * 8 * 0.9;
        const targetBitrateKbps = targetBitsTotal / effectiveDuration / 1000;
        const forcedScale = getForcedScalePercent();
        const recommendedScale = forcedScale ?? calculateOptimalResolution(
            targetSizeMb,
            effectiveDuration,
            sourceVideoWidth,
            sourceVideoHeight
        );
        const appliedScale = Math.max(10, Math.min(100, recommendedScale));
        const targetW = Math.floor(((sourceVideoWidth * appliedScale) / 100) / 2) * 2;
        const targetH = Math.floor(((sourceVideoHeight * appliedScale) / 100) / 2) * 2;

        let details = `Target bitrate: ~${Math.round(targetBitrateKbps)} kbps`;

        if (appliedScale < 100) {
            details += ` · Resolution: ${targetW}×${targetH} (${appliedScale}%)`;
        } else {
            details += ` · Resolution: ${sourceVideoWidth}×${sourceVideoHeight} (original)`;
        }

        if (videoSegments.length > 0 && effectiveDuration !== sourceDuration) {
            details += ` · Duration: ${effectiveDuration.toFixed(1)}s (edited)`;
        }

        outputSizeDetails = details;
    }

    async function handleUpload(event: MouseEvent) {
        event.stopPropagation();
        if (!selectedFile || !sourceDuration || !sourceVideoWidth || !sourceVideoHeight) {
            showStatus('Video metadata missing. Please re-select the file.', 'error');
            return;
        }

        uploadBtnDisabled = true;
        uploadBtnText = 'Uploading...';
        progressVisible = true;
        progressPercent = 10;
        canRetry = false;

        const formData = new FormData();
        formData.append('file', selectedFile);
        formData.append('codec', codecSelectValue);

        const targetSizeMb = parseFloat(outputSizeSliderValue.toString());
        const forcedScalePercent = getForcedScalePercent();
        const shouldForceResolution = forcedScalePercent !== null;
        const effectiveMaxSize = getEffectiveMaxSize(originalSizeMb, sourceDuration, videoSegments);
        
        formData.append('targetSizeMb', targetSizeMb.toFixed(2));
        const shouldSkipCompression = targetSizeMb >= effectiveMaxSize && !shouldForceResolution && !muteAudio;
        formData.append('skipCompression', shouldSkipCompression ? 'true' : 'false');
        formData.append('qualityMode', codecSelectValue === 'quality' ? 'true' : 'false');
        formData.append('muteAudio', muteAudio ? 'true' : 'false');

        const effectiveDuration = getEffectiveDuration(videoSegments, sourceDuration) ?? sourceDuration;

        if (shouldForceResolution && forcedScalePercent !== null) {
            formData.append('scalePercent', forcedScalePercent.toString());
        } else if (targetSizeMb < effectiveMaxSize) {
            const calculatedScalePercent = calculateOptimalResolution(
                targetSizeMb,
                effectiveDuration,
                sourceVideoWidth,
                sourceVideoHeight
            );

            if (Number.isFinite(calculatedScalePercent)) {
                formData.append('scalePercent', calculatedScalePercent.toString());
            }
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

        try {
            const result = await uploadVideo(formData);
            jobId = result.jobId;

            progressPercent = 100;
            isCompressing = true;
            showStatus('Video uploaded successfully. Processing...', 'processing');

            statusCheckInterval = window.setInterval(checkStatus, 2000);
        } catch (error) {
            console.error('Upload failed:', error);
            let errorMessage = (error as Error).message;

            if (errorMessage.includes('NetworkError') || errorMessage.includes('Failed to fetch')) {
                errorMessage =
                    'Network error: File may be too large or server is unreachable. Please try a smaller file or check your connection.';
            } else if (errorMessage.includes('413')) {
                errorMessage = 'File is too large. The server cannot accept files this big.';
            }

            showStatus('Upload failed: ' + errorMessage, 'error');
            uploadBtnDisabled = false;
            uploadBtnText = 'Process Video';
            progressVisible = false;
        }
    }

    async function checkStatus() {
        if (!jobId) return;
        try {
            const result = await getJobStatus(jobId);
                if (result.status === 'queued') {
                    showCancelButton = true;
                    canRetry = false;
                    const queueMsg =
                        result.queuePosition && result.queuePosition > 0
                            ? `Queued for processing (position ${result.queuePosition})...`
                            : 'Queued for processing...';
                    showStatus(queueMsg, 'processing');
                } else if (result.status === 'processing') {
                    showCancelButton = true;
                    canRetry = false;
                    const progressPercentValue = Math.max(10, Math.min(95, result.progress || 0));
                    progressPercent = progressPercentValue;

                    if (result.estimatedSecondsRemaining && result.estimatedSecondsRemaining > 0) {
                        showStatus(
                            `Processing video... ${progressPercentValue.toFixed(1)}% (ETA: ${formatTimeRemaining(
                                result.estimatedSecondsRemaining
                            )})`,
                            'processing'
                        );
                    } else {
                        showStatus(`Processing video... ${progressPercentValue.toFixed(1)}%`, 'processing');
                    }

                    outputMetadata = {
                        ...outputMetadata,
                        codec: result.codec || outputMetadata.codec,
                        encoderName: result.encoderName ?? outputMetadata.encoderName,
                        encoderIsHardware:
                            result.encoderIsHardware ?? outputMetadata.encoderIsHardware
                    };
                } else if (result.status === 'completed') {
                    if (statusCheckInterval) {
                        clearInterval(statusCheckInterval);
                        statusCheckInterval = null;
                    }
                    progressPercent = 100;
                    isCompressing = false;
                    showCancelButton = false;
                    downloadFileName = result.outputFilename || `compressed_${selectedFile?.name ?? jobId}`;
                    downloadMimeType = result.outputMimeType || 'video/mp4';

                    calculateOutputMetadata(result);
                    await loadVideoPreview();

                    showStatus('Processing complete! Preview and download your video.', 'success');
                    videoPreviewVisible = true;
                    downloadVisible = true;
                    progressVisible = false;
                    canRetry = false;
                } else if (result.status === 'cancelled') {
                    if (statusCheckInterval) {
                        clearInterval(statusCheckInterval);
                        statusCheckInterval = null;
                    }
                    isCompressing = false;
                    showCancelButton = false;
                    showStatus('Processing was cancelled.', 'error');
                    uploadBtnDisabled = false;
                    uploadBtnText = 'Process Video';
                    progressVisible = false;
                    canRetry = false;
                } else if (result.status === 'failed') {
                    if (statusCheckInterval) {
                        clearInterval(statusCheckInterval);
                        statusCheckInterval = null;
                    }
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
            if (statusCheckInterval) {
                clearInterval(statusCheckInterval);
                statusCheckInterval = null;
            }
            isCompressing = false;
            showStatus('Failed to check status: ' + (error as Error).message, 'error');
            uploadBtnDisabled = false;
            uploadBtnText = 'Process Video';
            progressVisible = false;
        }
    }

    async function handleCancelJob() {
        if (!jobId) return;

        if (!confirm('Are you sure you want to cancel this compression job?')) {
            return;
        }

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

            if (statusCheckInterval) {
                clearInterval(statusCheckInterval);
            }
            statusCheckInterval = window.setInterval(checkStatus, 2000);
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

    function calculateOutputMetadata(result: CompressionStatusResponse) {
        if (!originalSizeMb || !sourceDuration) return;

        const effectiveDuration = getEffectiveDuration(videoSegments, sourceDuration) ?? sourceDuration;
        const effectiveMaxSize = getEffectiveMaxSize(originalSizeMb, sourceDuration, videoSegments);

        const actualOutputBytes =
            typeof result.outputSizeBytes === 'number' &&
            Number.isFinite(result.outputSizeBytes) &&
            result.outputSizeBytes > 0
                ? result.outputSizeBytes
                : null;

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

    function handleClearResult() {
        if (videoPreviewUrl) {
            URL.revokeObjectURL(videoPreviewUrl);
            videoPreviewUrl = null;
        }

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
        setTimeout(() => {
            statusVisible = false;
        }, 3000);
    }

    async function loadVideoPreview() {
        if (!jobId) return;

        try {
            const response = await fetch(`/api/download/${jobId}`);
            if (response.ok) {
                const blob = await response.blob();
                if (videoPreviewUrl) {
                    URL.revokeObjectURL(videoPreviewUrl);
                }
                videoPreviewUrl = URL.createObjectURL(blob);

                const actualSizeMb = blob.size / (1024 * 1024);
                const effectiveMaxSize = getEffectiveMaxSize(originalSizeMb, sourceDuration, videoSegments);
                const compressionRatio = compressionSkipped
                    ? 0
                    : effectiveMaxSize > 0
                        ? (1 - actualSizeMb / effectiveMaxSize) * 100
                        : 0;
                outputMetadata = {
                    ...outputMetadata,
                    outputSizeBytes: blob.size,
                    outputSizeMb: actualSizeMb,
                    compressionRatio,
                    finalDuration: 0,
                    finalWidth: 0,
                    finalHeight: 0
                };
            } else {
                console.warn('Failed to load video preview');
            }
        } catch (error) {
            console.warn('Failed to load video preview:', error);
        }
    }

    function handleCompressedMetadata(
        event: CustomEvent<{ duration: number | null; width: number; height: number }>
    ) {
        const { duration, width, height } = event.detail;
        const sizeBytes = outputMetadata.outputSizeBytes;
        const bitrateKbps =
            duration && sizeBytes ? Math.round((sizeBytes * 8) / duration / 1000) : outputMetadata.videoBitrateKbps;

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
        if (downloadMimeType) {
            link.type = downloadMimeType;
        }
        document.body.appendChild(link);
        link.click();
        document.body.removeChild(link);

        resetInterface();
    }

    function showStatus(message: string, type: StatusMessageType) {
        statusMessage = message;
        statusType = type;
        statusVisible = true;
    }

    async function checkForUpdates(force = false) {
        if (!force && !autoUpdateEnabled) {
            return;
        }

        hasCheckedUpdates = true;
        try {
            const response = await fetch('/api/update');
            if (!response.ok) {
                return;
            }
            const payload: UpdateInfoPayload = await response.json();
            updateInfo = payload;
            showUpdateBanner = payload.updateAvailable === true;
        } catch (error) {
            console.warn('Update check failed', error);
        }
    }

    function dismissUpdateBanner() {
        showUpdateBanner = false;
    }

    function resetInterface() {
        if (statusCheckInterval) {
            clearInterval(statusCheckInterval);
            statusCheckInterval = null;
        }
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
        if (videoPreviewUrl) {
            URL.revokeObjectURL(videoPreviewUrl);
            videoPreviewUrl = null;
        }
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
        if (objectUrl) {
            URL.revokeObjectURL(objectUrl);
        }
        objectUrl = null;
        outputMetadata = createDefaultOutputMetadata();
        showVideoEditor = false;
        videoSegments = [];
    }

    $: finalBitrateLabel =
        outputMetadata.videoBitrateKbps > 0 ? `${Math.round(outputMetadata.videoBitrateKbps)} kbps` : '--';

    $: finalDurationLabel = formatDurationLabel(outputMetadata.finalDuration);

    $: resolutionPercent =
        sourceVideoWidth && sourceVideoHeight && outputMetadata.finalWidth > 0
            ? Math.round((outputMetadata.finalWidth / sourceVideoWidth) * 100)
            : null;

    $: finalResolutionLabel =
        outputMetadata.finalWidth > 0 && outputMetadata.finalHeight > 0
            ? `${outputMetadata.finalWidth}×${outputMetadata.finalHeight}${
                  resolutionPercent ? ` (${resolutionPercent}%)` : ''
              }`
            : '--';

    $: encodingTimeLabel =
        outputMetadata.encodingTime > 0 ? formatTimeRemaining(outputMetadata.encodingTime) : '--';
</script>

<div class="app-layout">
    <header class="app-header">
            <div class="header-title">
                <img class="app-logo" src="/logo.svg" alt="LiteClip logo" on:error={handleLogoError} />
                <h1>liteclip</h1>
            {#if updateInfo}
                <span class="version-chip">v{updateInfo.currentVersion}</span>
            {/if}
        </div>
        <div class="header-actions">
            <button class="icon-btn" type="button" on:click={() => (showSettingsModal = true)}>
                ⚙ settings
            </button>
        </div>
    </header>

    {#if showUpdateBanner && updateInfo?.updateAvailable}
        <div class="update-banner">
            <span>
                New version <strong>{updateInfo.latestVersion}</strong> is available.
            </span>
            <a
                class="update-link"
                href={updateInfo.downloadUrl || 'https://github.com/Guillermode20/smart-compressor/releases'}
                target="_blank"
                rel="noreferrer"
            >
                download
            </a>
            <button type="button" class="dismiss-btn" on:click={dismissUpdateBanner}>
                dismiss
            </button>
        </div>
    {/if}

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
                    <VideoEditor
                        videoFile={selectedFile}
                        onSegmentsChange={handleSegmentsChange}
                        onRemoveVideo={resetInterface}
                        savedSegments={videoSegments}
                    />
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
                    {finalBitrateLabel}
                    {finalResolutionLabel}
                    {finalDurationLabel}
                    encodingTimeLabel={encodingTimeLabel}
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
                sliderMax={sliderMaxRounded}
                sliderStep={sliderStepValue}
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

{#if !ffmpegReady}
    <div class="ffmpeg-overlay">
        <div class="ffmpeg-card">
            <h2>Preparing FFmpeg…</h2>
            <p class="ffmpeg-message">{ffmpegStatusMessage}</p>
            <div class="ffmpeg-progress">
                <div class="ffmpeg-progress-track">
                    <div
                        class="ffmpeg-progress-fill"
                        style={`width: ${Math.min(100, Math.max(5, ffmpegProgressPercent || 0)).toFixed(1)}%;`}
                    ></div>
                </div>
                <span>{Math.max(0, Math.min(100, ffmpegProgressPercent || 0)).toFixed(1)}%</span>
            </div>
            <span class="ffmpeg-state">{ffmpegState.toUpperCase()}</span>
            {#if ffmpegError}
                <p class="ffmpeg-error">{ffmpegError}</p>
                <button class="retry-btn" on:click={handleFfmpegRetry} disabled={ffmpegRetrying}>
                    {ffmpegRetrying ? 'retrying...' : 'retry download'}
                </button>
            {/if}
        </div>
    </div>
{/if}

<SettingsModal
    open={showSettingsModal}
    settings={userSettings}
    on:close={() => (showSettingsModal = false)}
    on:save={handleSettingsSave}
/>

