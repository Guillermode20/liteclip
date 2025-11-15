<script lang="ts">
    import VideoEditor from './VideoEditor.svelte';

    let selectedFile: File | null = null;
    let jobId: string | null = null;
    let statusCheckInterval: number | null = null;
    let downloadFileName: string | null = null;
    let downloadMimeType: string | null = null;
    let compressedVideoElement: HTMLVideoElement | null = null;
    
    let objectUrl: string | null = null;
    let sourceVideoWidth: number | null = null;
    let sourceVideoHeight: number | null = null;
    let sourceDuration: number | null = null;
    let originalSizeMb: number | null = null;
    
    let isDragover = false;
    let fileInfo = '';
    let metadataVisible = false;
    let metadataContent = '';
    let controlsVisible = false;
    let statusVisible = false;
    let statusMessage = '';
    let statusType: 'processing' | 'success' | 'error' = 'processing';
    let progressVisible = false;
    let progressPercent = 0;
    let isCompressing = false;
    let downloadVisible = false;
    let videoPreviewVisible = false;
    let videoPreviewUrl: string | null = null;
    let uploadBtnDisabled = true;
    let uploadBtnText = 'Upload & Compress Video';
    let outputSizeSliderDisabled = true;
    let outputSizeValue = '--';
    let outputSizeDetails = '';
    let outputSizeSliderValue = 100;
    let codecSelectValue = 'h265';
    let showCancelButton = false;
    let etaText = '';
    
    // Video editor state
    let showVideoEditor = false;
    let videoSegments: Array<{start: number, end: number}> = [];
    
    // Output metadata
    let outputMetadata = {
        outputSizeBytes: 0,
        outputSizeMb: 0,
        compressionRatio: 0,
        targetBitrateKbps: 0,
        videoBitrateKbps: 0,
        estimatedVideoBitrateKbps: 0,
        scalePercent: 100,
        codec: 'h264',
        encoderName: null,
        encoderIsHardware: false,
        encodingTime: 0,
        finalDuration: 0,
        finalWidth: 0,
        finalHeight: 0,
    };
    
    const codecDetails = {
        h264: {
            helper: 'Best compatibility across browsers and devices.',
            container: 'mp4',
        },
        h265: {
            helper: 'Higher efficiency than H.264 but slower to encode. Limited support on older devices.',
            container: 'mp4',
        },
        vp9: {
            helper: 'Great for modern browsers. Outputs WebM files optimized for streaming.',
            container: 'webm',
        },
        av1: {
            helper: 'Smallest files but slowest encode. Requires very recent hardware/software.',
            container: 'webm',
        },
    };
    
    let codecHelperText = codecDetails.h264.helper;
    
    function formatFileSize(bytes: number): string {
        if (bytes === 0) return '0 Bytes';
        const k = 1024;
        const sizes = ['Bytes', 'KB', 'MB', 'GB'];
        const i = Math.floor(Math.log(bytes) / Math.log(k));
        return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
    }

    function formatDurationLabel(seconds: number | null): string {
        if (!seconds || !isFinite(seconds)) {
            return '--';
        }
        return `${seconds.toFixed(1)}s`;
    }
    
    function handleFileSelect(file: File) {
        if (!file.type.startsWith('video/')) {
            alert('Please select a video file');
            return;
        }
        
        selectedFile = file;
        originalSizeMb = selectedFile.size / (1024 * 1024);
        fileInfo = `Selected: ${file.name} (${formatFileSize(file.size)})`;
        uploadBtnDisabled = false;
        uploadBtnText = 'Upload & Compress Video';
        controlsVisible = true;
        metadataVisible = false;
        showVideoEditor = true;
        
        outputSizeSliderDisabled = true;
        outputSizeValue = '--';
        outputSizeDetails = 'Reading video metadata...';
        updateCodecHelper();
        
        // Load metadata
        if (objectUrl) {
            URL.revokeObjectURL(objectUrl);
        }
        objectUrl = URL.createObjectURL(file);
        const videoEl = document.createElement('video');
        videoEl.preload = 'metadata';
        videoEl.src = objectUrl;
        videoEl.addEventListener('loadedmetadata', () => {
            sourceVideoWidth = videoEl.videoWidth || null;
            sourceVideoHeight = videoEl.videoHeight || null;
            const duration = isFinite(videoEl.duration) ? videoEl.duration : null;
            sourceDuration = duration;
            const kbps = duration ? Math.round((file.size * 8) / duration / 1000) : null;
            const dimsText = (sourceVideoWidth && sourceVideoHeight) ? `${sourceVideoWidth}×${sourceVideoHeight}` : 'Unknown';
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
            
            // Configure slider
            outputSizeSliderValue = 100;
            outputSizeSliderDisabled = false;
            updateOutputSizeDisplay();
        }, { once: true });
    }
    
    function handleDragOver(event: DragEvent) {
        event.preventDefault();
        isDragover = true;
    }
    
    function handleDragLeave() {
        isDragover = false;
    }

    function handleDrop(event: DragEvent) {
        event.preventDefault();
        isDragover = false;
        const files = event.dataTransfer?.files;
        if (files && files.length > 0) {
            handleFileSelect(files[0]);
        }
    }

    function handleFileInputChange(event: Event) {
        const target = event.target as HTMLInputElement;
        if (target.files && target.files.length > 0) {
            handleFileSelect(target.files[0]);
        }
    }

    function triggerFileInput() {
        document.getElementById('fileInput')?.click();
    }
    
    function updateCodecHelper() {
        const details = codecDetails[codecSelectValue as keyof typeof codecDetails];
        if (details) {
            codecHelperText = details.helper;
        } else {
            codecHelperText = '';
        }
    }
    
    function handleSegmentsChange(segments: Array<{start: number, end: number}>) {
        videoSegments = segments;
        
        // Always recalculate output size display when segments change
        // This ensures bitrate estimates match the edited duration
        updateOutputSizeDisplay();
    }
    
    function getEffectiveDuration(): number | null {
        // If we have segments, calculate the total edited duration
        if (videoSegments && videoSegments.length > 0) {
            return videoSegments.reduce((sum, seg) => sum + (seg.end - seg.start), 0);
        }
        // Otherwise use the original duration
        return sourceDuration;
    }
    
    function getEffectiveMaxSize(): number {
        // Calculate the max target size based on the edited duration
        if (!originalSizeMb || !sourceDuration || sourceDuration <= 0) {
            return originalSizeMb || 0;
        }
        
        const effectiveDuration = getEffectiveDuration();
        if (!effectiveDuration || effectiveDuration === sourceDuration) {
            return originalSizeMb;
        }
        
        // Scale the original size by the duration ratio
        // (edited duration / original duration) * original size
        const durationRatio = effectiveDuration / sourceDuration;
        return originalSizeMb * durationRatio;
    }
    
    function calculateOptimalResolution(targetSizeMb: number, durationSec: number, width: number, height: number): number {
        // Validate inputs
        if (!Number.isFinite(targetSizeMb) || !Number.isFinite(durationSec) || 
            !Number.isFinite(width) || !Number.isFinite(height) || 
            targetSizeMb <= 0 || durationSec <= 0 || width <= 0 || height <= 0) {
            return 100; // Return 100% scale if inputs are invalid
        }
        
        // Updated to match backend: 97% container overhead factor (3% overhead) for MP4
        const targetBitsTotal = (targetSizeMb * 1024 * 1024 * 8 * 0.97);
        const targetBitrateKbps = targetBitsTotal / durationSec / 1000;
        const videoBitrateKbps = targetBitrateKbps - 128;
        
        const pixels = width * height;
        const bitsPerPixel = (videoBitrateKbps * 1000) / pixels / 30;
        
        if (bitsPerPixel >= 0.1) {
            return 100;
        }
        
        const targetBpp = 0.1;
        const scaleFactor = Math.sqrt(bitsPerPixel / targetBpp);
        let scalePercent = Math.min(100, Math.round(scaleFactor * 100));
        
        const minHeight = 480;
        const heightScalePercent = Math.round((minHeight / height) * 100);
        scalePercent = Math.max(scalePercent, heightScalePercent);
        
        scalePercent = Math.max(25, scalePercent);
        
        return scalePercent;
    }
    
    function updateOutputSizeDisplay() {
        if (!originalSizeMb || !Number.isFinite(originalSizeMb)) {
            outputSizeValue = '--';
            outputSizeDetails = '';
            return;
        }
        
        const percent = parseFloat(outputSizeSliderValue.toString());
        const effectiveMaxSize = getEffectiveMaxSize();
        const targetSizeMb = (effectiveMaxSize * percent) / 100;
        
        const displayValue = targetSizeMb >= 10 ? targetSizeMb.toFixed(0) : targetSizeMb.toFixed(1);
        outputSizeValue = `${displayValue} MB`;
        
        // Show max size if it's different from original
        if (videoSegments && videoSegments.length > 0 && effectiveMaxSize !== originalSizeMb) {
            outputSizeValue += ` (max: ${effectiveMaxSize.toFixed(1)} MB)`;
        }
        
        if (!sourceDuration || !sourceVideoWidth || !sourceVideoHeight) {
            outputSizeDetails = 'Waiting for video metadata...';
            return;
        }
        
        // Use effective duration (edited duration if segments exist, otherwise original)
        const effectiveDuration = getEffectiveDuration() || sourceDuration;
        
        // If target is 100% or more, indicate no compression will occur
        if (percent >= 100) {
            outputSizeDetails = videoSegments && videoSegments.length > 0 
                ? 'Will cut video segments only (no compression)'
                : 'No compression (original quality preserved)';
            return;
        }
        
        const targetBitsTotal = (targetSizeMb * 1024 * 1024 * 8 * 0.9);
        const targetBitrateKbps = targetBitsTotal / effectiveDuration / 1000;
        const videoBitrateKbps = Math.max(100, targetBitrateKbps - 128);
        
        const recommendedScale = calculateOptimalResolution(targetSizeMb, effectiveDuration, sourceVideoWidth, sourceVideoHeight);
        
        const targetW = Math.floor((sourceVideoWidth * recommendedScale / 100) / 2) * 2;
        const targetH = Math.floor((sourceVideoHeight * recommendedScale / 100) / 2) * 2;
        
        let details = `Target bitrate: ~${Math.round(targetBitrateKbps)} kbps`;
        
        if (recommendedScale < 100) {
            details += ` · Resolution: ${targetW}×${targetH} (${recommendedScale}%)`;
        } else {
            details += ` · Resolution: ${sourceVideoWidth}×${sourceVideoHeight} (original)`;
        }
        
        // Show edited duration if segments are active
        if (videoSegments && videoSegments.length > 0 && effectiveDuration !== sourceDuration) {
            details += ` · Duration: ${effectiveDuration.toFixed(1)}s (edited)`;
        }
        
        outputSizeDetails = details;
    }
    
    function handlePresetClick(targetPercent: string) {
        if (outputSizeSliderDisabled || !originalSizeMb) return;
        outputSizeSliderValue = parseFloat(targetPercent);
        updateOutputSizeDisplay();
    }
    
    async function handleUpload(event: MouseEvent) {
        event.stopPropagation();
        if (!selectedFile) return;
        
        uploadBtnDisabled = true;
        uploadBtnText = 'Uploading...';
        progressVisible = true;
        progressPercent = 10;
        
        const formData = new FormData();
        formData.append('file', selectedFile);
        formData.append('codec', codecSelectValue);
        
        const percent = parseFloat(outputSizeSliderValue.toString());
        const effectiveMaxSize = getEffectiveMaxSize();
        const targetSizeMb = (effectiveMaxSize * percent) / 100;
        formData.append('targetSizeMb', targetSizeMb.toFixed(2));
        
        // Use effective duration for calculations
        const effectiveDuration = getEffectiveDuration() || sourceDuration!;
        
        // Only calculate scale if we're actually compressing (< 100%)
        if (percent < 100) {
            const calculatedScalePercent = calculateOptimalResolution(
                targetSizeMb,
                effectiveDuration,
                sourceVideoWidth!,
                sourceVideoHeight!
            );
            
            // Only append scalePercent if it's a valid number
            if (Number.isFinite(calculatedScalePercent)) {
                formData.append('scalePercent', calculatedScalePercent.toString());
            }
        } else {
            // At 100%, don't scale (preserve original resolution)
            formData.append('scalePercent', '100');
        }
        
        if (sourceDuration && isFinite(sourceDuration)) {
            formData.append('sourceDuration', sourceDuration.toFixed(3));
        }
        if (Number.isFinite(sourceVideoWidth) && sourceVideoWidth! > 0) {
            formData.append('sourceWidth', sourceVideoWidth!.toString());
        }
        if (Number.isFinite(sourceVideoHeight) && sourceVideoHeight! > 0) {
            formData.append('sourceHeight', sourceVideoHeight!.toString());
        }
        formData.append('originalSizeBytes', selectedFile.size.toString());
        
        // Add video segments - always send if we have them
        if (videoSegments && videoSegments.length > 0) {
            formData.append('segments', JSON.stringify(videoSegments));
        }
        
        try {
            const response = await fetch('/api/compress', {
                method: 'POST',
                body: formData
            });
            
            if (!response.ok) {
                let errorMsg = `Server error (${response.status})`;
                try {
                    const errorText = await response.text();
                    // Try to parse as JSON
                    try {
                        const errorData = JSON.parse(errorText);
                        errorMsg = errorData.error || errorData.detail || errorMsg;
                    } catch {
                        // Not JSON, use the text as-is
                        errorMsg = errorText || errorMsg;
                    }
                } catch (e) {
                    // If we can't read the body at all, use the default message
                    errorMsg = `Server error (${response.status})`;
                }
                throw new Error(errorMsg);
            }
            
            const result = await response.json();
            jobId = result.jobId;
            
            progressPercent = 100;
            isCompressing = true;
            showStatus('Video uploaded successfully. Compressing...', 'processing');
            
            statusCheckInterval = window.setInterval(checkStatus, 2000);
            
        } catch (error) {
            console.error('Upload failed:', error);
            let errorMessage = (error as Error).message;
            
            if (errorMessage.includes('NetworkError') || errorMessage.includes('Failed to fetch')) {
                errorMessage = 'Network error: File may be too large or server is unreachable. Please try a smaller file or check your connection.';
            } else if (errorMessage.includes('413')) {
                errorMessage = 'File is too large. The server cannot accept files this big.';
            }
            
            showStatus('Upload failed: ' + errorMessage, 'error');
            uploadBtnDisabled = false;
            uploadBtnText = 'Upload & Compress Video';
            progressVisible = false;
        }
    }
    
    async function checkStatus() {
        try {
            const response = await fetch(`/api/status/${jobId}`);
            if (response.ok) {
                const result = await response.json();
                if (result.status === 'queued') {
                    showCancelButton = true;
                    const queueMsg = result.queuePosition > 0 
                        ? `Queued for compression (position ${result.queuePosition})...`
                        : 'Queued for compression...';
                    showStatus(queueMsg, 'processing');
                } else if (result.status === 'processing') {
                    showCancelButton = true;
                    const progressPercentValue = Math.max(10, Math.min(95, result.progress || 0));
                    progressPercent = progressPercentValue;
                    
                    // Update ETA
                    if (result.estimatedSecondsRemaining && result.estimatedSecondsRemaining > 0) {
                        etaText = formatTimeRemaining(result.estimatedSecondsRemaining);
                        showStatus(`Compressing video... ${progressPercentValue.toFixed(1)}% (ETA: ${etaText})`, 'processing');
                    } else {
                        etaText = '';
                        showStatus(`Compressing video... ${progressPercentValue.toFixed(1)}%`, 'processing');
                    }
                    // If encoder/codec info is available while processing, surface it in the UI
                    try {
                        outputMetadata.codec = result.codec || outputMetadata.codec;
                        outputMetadata.encoderName = result.encoderName || outputMetadata.encoderName;
                        outputMetadata.encoderIsHardware = result.encoderIsHardware ?? outputMetadata.encoderIsHardware;
                    } catch (e) {
                        // ignore if outputMetadata isn't available for some reason
                    }
                } else if (result.status === 'completed') {
                    if (statusCheckInterval) {
                        clearInterval(statusCheckInterval);
                        statusCheckInterval = null;
                    }
                    progressPercent = 100;
                    isCompressing = false;
                    showCancelButton = false;
                    etaText = '';
                    downloadFileName = result.outputFilename || `compressed_${selectedFile?.name ?? jobId}`;
                    downloadMimeType = result.outputMimeType || 'video/mp4';

                    // Calculate and store output metadata
                    calculateOutputMetadata(result);

                    // Load video for preview
                    loadVideoPreview();

                    showStatus('Compression complete! Preview and download your video.', 'success');
                    videoPreviewVisible = true;
                    downloadVisible = true;
                    progressVisible = false;
                } else if (result.status === 'cancelled') {
                    if (statusCheckInterval) {
                        clearInterval(statusCheckInterval);
                        statusCheckInterval = null;
                    }
                    isCompressing = false;
                    showCancelButton = false;
                    etaText = '';
                    showStatus('Compression was cancelled.', 'error');
                    uploadBtnDisabled = false;
                    uploadBtnText = 'Upload & Compress Video';
                    progressVisible = false;
                } else if (result.status === 'failed') {
                    if (statusCheckInterval) {
                        clearInterval(statusCheckInterval);
                        statusCheckInterval = null;
                    }
                    isCompressing = false;
                    showCancelButton = false;
                    etaText = '';
                    showStatus('Compression failed: ' + (result.message || 'Unknown error'), 'error');
                    uploadBtnDisabled = false;
                    uploadBtnText = 'Upload & Compress Video';
                    progressVisible = false;
                }
            } else {
                if (statusCheckInterval) {
                    clearInterval(statusCheckInterval);
                    statusCheckInterval = null;
                }
                isCompressing = false;
                let errorMessage = 'Unknown error';
                try {
                    const errorText = await response.text();
                    try {
                        const errorData = JSON.parse(errorText);
                        errorMessage = errorData.error || errorMessage;
                    } catch {
                        errorMessage = errorText || errorMessage;
                    }
                } catch {
                    // If we can't read the body, use default message
                }
                showStatus('Compression failed: ' + errorMessage, 'error');
                uploadBtnDisabled = false;
                uploadBtnText = 'Upload & Compress Video';
                progressVisible = false;
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
            uploadBtnText = 'Upload & Compress Video';
            progressVisible = false;
        }
    }
    
    async function handleCancelJob() {
        if (!jobId) return;
        
        if (!confirm('Are you sure you want to cancel this compression job?')) {
            return;
        }

        try {
            const response = await fetch(`/api/cancel/${jobId}`, {
                method: 'POST'
            });

            if (response.ok) {
                showStatus('Cancelling compression...', 'processing');
                // Status check will handle the rest
            } else {
                const error = await response.json();
                showStatus('Failed to cancel: ' + (error.error || 'Unknown error'), 'error');
            }
        } catch (error) {
            console.error('Cancel failed:', error);
            showStatus('Failed to cancel compression', 'error');
        }
    }

    function formatTimeRemaining(seconds: number): string {
        if (seconds < 60) {
            return `${seconds}s`;
        } else if (seconds < 3600) {
            const minutes = Math.floor(seconds / 60);
            const secs = seconds % 60;
            return secs > 0 ? `${minutes}m ${secs}s` : `${minutes}m`;
        } else {
            const hours = Math.floor(seconds / 3600);
            const minutes = Math.floor((seconds % 3600) / 60);
            return minutes > 0 ? `${hours}h ${minutes}m` : `${hours}h`;
        }
    }
    
    function calculateOutputMetadata(result: any) {
        if (!originalSizeMb || !sourceDuration) return;
        
        // Use edited duration if segments exist
        const effectiveDuration = getEffectiveDuration() || sourceDuration;
        const effectiveMaxSize = getEffectiveMaxSize();
        
        // Estimate output size based on available data
        let estimatedOutputBytes = 0;
        let outputSizeMb = 0;
        
        if (result.targetBitrateKbps && result.targetBitrateKbps > 0) {
            estimatedOutputBytes = Math.round((result.targetBitrateKbps * 1000 * effectiveDuration) / 8);
            outputSizeMb = estimatedOutputBytes / (1024 * 1024);
        } else if (result.videoBitrateKbps && result.videoBitrateKbps > 0) {
            // Account for audio bitrate (128 kbps default)
            const totalBitrateKbps = result.videoBitrateKbps + 128;
            estimatedOutputBytes = Math.round((totalBitrateKbps * 1000 * effectiveDuration) / 8);
            outputSizeMb = estimatedOutputBytes / (1024 * 1024);
        } else {
            // No bitrate info (compression was skipped) - use effective max size as estimate
            // The actual size will be updated when the video preview loads
            outputSizeMb = effectiveMaxSize;
            estimatedOutputBytes = Math.round(effectiveMaxSize * 1024 * 1024);
        }
        
        // Calculate compression ratio based on the effective max size (edited video size)
        const safeEffectiveMaxSize = effectiveMaxSize > 0 ? effectiveMaxSize : outputSizeMb || 1;
        const compressionRatio = (1 - outputSizeMb / safeEffectiveMaxSize) * 100;
        const startTime = new Date(result.createdAt || Date.now());
        const completionTime = new Date(result.completedAt || Date.now());
        const encodingSeconds = Math.max(0, (completionTime.getTime() - startTime.getTime()) / 1000);
        
        outputMetadata = {
            outputSizeBytes: estimatedOutputBytes,
            outputSizeMb: outputSizeMb,
            compressionRatio: compressionRatio,
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
            finalHeight: 0,
        };
    }
    
    function handleClearResult() {
        // Clean up video preview URL
        if (videoPreviewUrl) {
            URL.revokeObjectURL(videoPreviewUrl);
            videoPreviewUrl = null;
        }
        
        // Reset all state
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
        etaText = '';
        
        // Reset output metadata
        outputMetadata = {
            outputSizeBytes: 0,
            outputSizeMb: 0,
            compressionRatio: 0,
            targetBitrateKbps: 0,
            videoBitrateKbps: 0,
            estimatedVideoBitrateKbps: 0,
            scalePercent: 100,
            codec: 'h264',
            encoderName: null,
            encoderIsHardware: false,
            encodingTime: 0,
            finalDuration: 0,
            finalWidth: 0,
            finalHeight: 0,
        };
        
        // Re-enable upload button to try again with different settings
        uploadBtnDisabled = false;
        uploadBtnText = 'Upload & Compress Video';
        
        // Show success message
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
                
                // Update metadata with actual file size
                const actualSizeMb = blob.size / (1024 * 1024);
                const effectiveMaxSize = getEffectiveMaxSize();
                const compressionRatio = effectiveMaxSize > 0
                    ? (1 - actualSizeMb / effectiveMaxSize) * 100
                    : 0;
                outputMetadata = {
                    ...outputMetadata,
                    outputSizeBytes: blob.size,
                    outputSizeMb: actualSizeMb,
                    compressionRatio: compressionRatio,
                    finalDuration: 0,
                    finalWidth: 0,
                    finalHeight: 0,
                };
            } else {
                console.warn('Failed to load video preview');
            }
        } catch (error) {
            console.warn('Failed to load video preview:', error);
        }
    }

    function handleCompressedMetadata() {
        if (!compressedVideoElement) return;
        const duration = isFinite(compressedVideoElement.duration) ? compressedVideoElement.duration : null;
        const width = compressedVideoElement.videoWidth || 0;
        const height = compressedVideoElement.videoHeight || 0;
        const sizeBytes = outputMetadata.outputSizeBytes;
        const bitrateKbps = duration && sizeBytes
            ? Math.round((sizeBytes * 8) / duration / 1000)
            : outputMetadata.videoBitrateKbps;

        outputMetadata = {
            ...outputMetadata,
            videoBitrateKbps: bitrateKbps,
            estimatedVideoBitrateKbps: bitrateKbps,
            finalDuration: duration || outputMetadata.finalDuration,
            finalWidth: width,
            finalHeight: height,
        };
    }

    function handleDownload() {
        if (jobId) {
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
    }
    
    function showStatus(message: string, type: 'processing' | 'success' | 'error') {
        statusMessage = message;
        statusType = type;
        statusVisible = true;
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
        uploadBtnText = 'Upload & Compress Video';
        controlsVisible = false;
        metadataVisible = false;
        downloadFileName = null;
        downloadMimeType = null;
        showCancelButton = false;
        etaText = '';
        if (videoPreviewUrl) {
            URL.revokeObjectURL(videoPreviewUrl);
            videoPreviewUrl = null;
        }
        outputSizeSliderDisabled = true;
        outputSizeSliderValue = 100;
        outputSizeValue = '--';
        outputSizeDetails = '';
        codecSelectValue = 'h265';
        updateCodecHelper();
        sourceVideoWidth = null;
        sourceVideoHeight = null;
        sourceDuration = null;
        originalSizeMb = null;
        if (objectUrl) {
            URL.revokeObjectURL(objectUrl);
        }
        objectUrl = null;
        compressedVideoElement = null;
        outputMetadata = {
            outputSizeBytes: 0,
            outputSizeMb: 0,
            compressionRatio: 0,
            targetBitrateKbps: 0,
            videoBitrateKbps: 0,
            estimatedVideoBitrateKbps: 0,
            scalePercent: 100,
            codec: 'h264',
            encoderName: null,
            encoderIsHardware: false,
            encodingTime: 0,
            finalDuration: 0,
            finalWidth: 0,
            finalHeight: 0,
        };
        showVideoEditor = false;
        videoSegments = [];
    }

    $: finalBitrateLabel = outputMetadata.videoBitrateKbps > 0
        ? `${Math.round(outputMetadata.videoBitrateKbps)} kbps`
        : '--';

    $: finalDurationLabel = formatDurationLabel(outputMetadata.finalDuration);

    $: resolutionPercent = (() => {
        if (!sourceVideoWidth || !sourceVideoHeight || outputMetadata.finalWidth <= 0) {
            return null;
        }
        return Math.round((outputMetadata.finalWidth / sourceVideoWidth) * 100);
    })();

    $: finalResolutionLabel = (outputMetadata.finalWidth > 0 && outputMetadata.finalHeight > 0)
        ? `${outputMetadata.finalWidth}×${outputMetadata.finalHeight}${resolutionPercent ? ` (${resolutionPercent}%)` : ''}`
        : '--';
</script>

<div class="app-layout">
    <!-- Header -->
    <header class="app-header">
        <h1>// liteclip</h1>
        <p class="subtitle">fast video compression with intelligent optimization</p>
    </header>

    <!-- Main Layout -->
    <div class="main-layout">
        <!-- Main Content -->
        <main class="main-content">
            <!-- Upload Section (always visible) -->
            {#if !videoPreviewVisible && !progressVisible}
                <div class="content-card">
                    <h2 class="section-title">// upload_video</h2>
                    <div 
                        class="upload-area" 
                        class:dragover={isDragover}
                        class:has-video={selectedFile && objectUrl}
                        on:dragover={handleDragOver}
                        on:dragleave={handleDragLeave}
                        on:drop={handleDrop}
                        role="region"
                        aria-label="Video upload area"
                    >
                        <input 
                            type="file" 
                            id="fileInput" 
                            accept="video/*" 
                            style="display: none;"
                            on:change={handleFileInputChange}
                        />
                        
                        {#if selectedFile && objectUrl && controlsVisible}
                            <div 
                                class="upload-state"
                                on:click={triggerFileInput}
                                on:keydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); triggerFileInput(); } }}
                                role="button"
                                tabindex="0"
                            >
                                <div class="upload-state-text">
                                    <p class="upload-state-title">Video ready for editing</p>
                                    <p class="upload-state-helper">Drop another file to replace it</p>
                                </div>
                                {#if fileInfo}
                                    <div class="file-info">{fileInfo}</div>
                                {/if}
                            </div>
                        {:else}
                            <div 
                                class="upload-prompt"
                                on:click={triggerFileInput}
                                on:keydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); triggerFileInput(); } }}
                                role="button"
                                tabindex="0"
                            >
                                <svg class="upload-icon" xmlns="http://www.w3.org/2000/svg" width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                    <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path>
                                    <polyline points="17 8 12 3 7 8"></polyline>
                                    <line x1="12" y1="3" x2="12" y2="15"></line>
                                </svg>
                                <p class="upload-text">Drop video or click to select</p>
                            </div>
                        {/if}
                    </div>
                </div>
            {/if}

            <!-- Video Editor -->
            {#if showVideoEditor && selectedFile && !videoPreviewVisible && !progressVisible}
                <div class="content-card">
                    <VideoEditor 
                        videoFile={selectedFile} 
                        onSegmentsChange={handleSegmentsChange}
                        onRemoveVideo={resetInterface}
                    />
                </div>
            {/if}

            {#if progressVisible}
                <div class="content-card">
                    <h2 class="section-title">// processing</h2>
                    <div class="progress-container">
                        <div class="progress-bar">
                            <div class="progress-fill" class:compressing={isCompressing} style="width: {progressPercent}%;"></div>
                        </div>
                        <div class="progress-text">{progressPercent.toFixed(1)}%</div>
                    </div>
                </div>
            {/if}

            {#if statusVisible}
                <div class="content-card status-card status-{statusType}">
                    <div class="status-icon">
                        {#if statusType === 'processing'}
                            <svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                <circle cx="12" cy="12" r="10"></circle>
                                <polyline points="12 6 12 12 16 14"></polyline>
                            </svg>
                        {:else if statusType === 'success'}
                            <svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                <polyline points="20 6 9 17 4 12"></polyline>
                            </svg>
                        {:else}
                            <svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                <circle cx="12" cy="12" r="10"></circle>
                                <line x1="15" y1="9" x2="9" y2="15"></line>
                                <line x1="9" y1="9" x2="15" y2="15"></line>
                            </svg>
                        {/if}
                    </div>
                    <div class="status-message">{statusMessage}</div>
                </div>
            {/if}

            {#if videoPreviewVisible}
                <div class="content-card">
                    <h2 class="section-title">// compressed_output</h2>
                    <div class="video-container">
                        <video
                            bind:this={compressedVideoElement}
                            controls
                            preload="none"
                            aria-label="Compressed video preview"
                            tabindex="0"
                            on:loadedmetadata={handleCompressedMetadata}
                        >
                            {#if videoPreviewUrl}
                                <source src={videoPreviewUrl} type={downloadMimeType || 'video/mp4'}>
                            {/if}
                            <track kind="captions" srclang="en" label="English" default>
                            Your browser does not support the video tag.
                        </video>
                    </div>
                    
                    <!-- Output Metadata -->
                    <div class="metadata-section">
                        <h3 class="metadata-title">// compression_stats</h3>
                        <div class="metadata-grid">
                            <div class="metadata-item">
                                <span class="metadata-label">output_size</span>
                                <span class="metadata-value">{outputMetadata.outputSizeMb.toFixed(1)} MB</span>
                            </div>
                            <div class="metadata-item">
                                <span class="metadata-label">compression</span>
                                <span class="metadata-value" class:positive={outputMetadata.compressionRatio > 0}>
                                    {outputMetadata.compressionRatio.toFixed(1)}%
                                </span>
                            </div>
                            <div class="metadata-item">
                                <span class="metadata-label">codec</span>
                                <span class="metadata-value">
                                    {outputMetadata.codec.toUpperCase()}
                                    {#if outputMetadata.encoderName}
                                        &nbsp;—&nbsp;{outputMetadata.encoderName}{outputMetadata.encoderIsHardware ? ' (hardware)' : ' (software)'}
                                    {/if}
                                </span>
                            </div>
                            <div class="metadata-item">
                                <span class="metadata-label">bitrate</span>
                                <span class="metadata-value">{finalBitrateLabel}</span>
                            </div>
                            <div class="metadata-item">
                                <span class="metadata-label">resolution</span>
                                <span class="metadata-value">{finalResolutionLabel}</span>
                            </div>
                            <div class="metadata-item">
                                <span class="metadata-label">duration</span>
                                <span class="metadata-value">{finalDurationLabel}</span>
                            </div>
                            <div class="metadata-item">
                                <span class="metadata-label">original_size</span>
                                <span class="metadata-value">{originalSizeMb?.toFixed(1)} MB</span>
                            </div>
                            <div class="metadata-item">
                                <span class="metadata-label">encoding_time</span>
                                <span class="metadata-value">{formatTimeRemaining(outputMetadata.encodingTime)}</span>
                            </div>
                        </div>
                    </div>
                    
                    <div class="action-buttons">
                        <button id="downloadBtn" on:click={handleDownload} class="action-btn primary">
                            $ download_compressed_video
                        </button>
                        <button 
                            id="clearBtn"
                            on:click={handleClearResult}
                            class="action-btn secondary"
                        >
                            $ clear_and_compress_again
                        </button>
                    </div>
                </div>
            {/if}
        </main>

        <!-- Sidebar - Only display when a video is selected -->
        {#if selectedFile}
            <aside class="sidebar">
                <div class="sidebar-content">

                    <!-- Metadata Section -->
                    {#if metadataVisible}
                        <section class="sidebar-section">
                            <h2 class="section-title">// file_info</h2>
                            <div class="metadata">
                                {@html metadataContent}
                            </div>
                        </section>
                    {/if}

                    <!-- Settings Section -->
                    {#if controlsVisible}
                        <section class="sidebar-section">
                            <h2 class="section-title">// settings</h2>
                            <div class="settings-group">
                                <label for="outputSizeSlider" class="setting-label">
                                    <strong>target_size</strong>
                                    <span class="setting-value">{outputSizeValue}</span>
                                </label>
                                <div class="preset-buttons">
                                    <button type="button" class="preset-btn" on:click={() => handlePresetClick('25')}>-75%</button>
                                    <button type="button" class="preset-btn" on:click={() => handlePresetClick('50')}>-50%</button>
                                    <button type="button" class="preset-btn" on:click={() => handlePresetClick('75')}>-25%</button>
                                </div>
                                <input 
                                    type="range" 
                                    id="outputSizeSlider" 
                                    min="1" 
                                    max="100" 
                                    step="0.5" 
                                    bind:value={outputSizeSliderValue}
                                    disabled={outputSizeSliderDisabled}
                                    on:input={updateOutputSizeDisplay}
                                    class="size-slider"
                                />
                                <div class="helper-text">// drag to adjust compression</div>
                                {#if outputSizeDetails}
                                    <div class="estimate-line">→ {outputSizeDetails}</div>
                                {/if}
                            </div>

                            <div class="settings-group">
                                <label for="codecSelect" class="setting-label">
                                    <strong>codec</strong>
                                </label>
                                <select id="codecSelect" bind:value={codecSelectValue} on:change={() => { updateCodecHelper(); updateOutputSizeDisplay(); }}>
                                    <option value="h264">h264 (mp4)</option>
                                    <option value="h265">h265 / hevc (mp4)</option>
                                </select>
                                {#if codecHelperText}
                                    <div class="helper-text">// {codecHelperText}</div>
                                {/if}
                            </div>
                        </section>

                        <!-- Action Buttons -->
                        <section class="sidebar-section">
                            {#if !videoPreviewVisible}
                                <button 
                                    id="uploadBtn" 
                                    disabled={uploadBtnDisabled}
                                    on:click={handleUpload}
                                    class="action-btn primary"
                                >
                                    $ {uploadBtnText.toLowerCase().replace('&', '+')}
                                </button>
                            {/if}

                            {#if showCancelButton}
                                <button 
                                    id="cancelBtn"
                                    on:click={handleCancelJob}
                                    class="action-btn danger"
                                >
                                    $ cancel compression
                                </button>
                            {/if}
                        </section>
                    {/if}
                </div>
            </aside>
        {/if}
    </div>
</div>
