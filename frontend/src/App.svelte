<script lang="ts">
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
    let codecSelectValue = 'h264';
    let showCancelButton = false;
    let etaText = '';
    
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
    
    function updateCodecHelper() {
        const details = codecDetails[codecSelectValue as keyof typeof codecDetails];
        if (details) {
            codecHelperText = details.helper;
        } else {
            codecHelperText = '';
        }
    }
    
    function calculateOptimalResolution(targetSizeMb: number, durationSec: number, width: number, height: number): number {
        const targetBitsTotal = (targetSizeMb * 1024 * 1024 * 8 * 0.9);
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
        const targetSizeMb = (originalSizeMb * percent) / 100;
        
        const displayValue = targetSizeMb >= 10 ? targetSizeMb.toFixed(0) : targetSizeMb.toFixed(1);
        outputSizeValue = `${displayValue} MB (${percent.toFixed(0)}% of original)`;
        
        if (!sourceDuration || !sourceVideoWidth || !sourceVideoHeight) {
            outputSizeDetails = 'Waiting for video metadata...';
            return;
        }
        
        const targetBitsTotal = (targetSizeMb * 1024 * 1024 * 8 * 0.9);
        const targetBitrateKbps = targetBitsTotal / sourceDuration / 1000;
        const videoBitrateKbps = Math.max(100, targetBitrateKbps - 128);
        
        const recommendedScale = calculateOptimalResolution(targetSizeMb, sourceDuration, sourceVideoWidth, sourceVideoHeight);
        
        const targetW = Math.floor((sourceVideoWidth * recommendedScale / 100) / 2) * 2;
        const targetH = Math.floor((sourceVideoHeight * recommendedScale / 100) / 2) * 2;
        
        let details = `Target bitrate: ~${Math.round(targetBitrateKbps)} kbps`;
        
        if (recommendedScale < 100) {
            details += ` · Resolution: ${targetW}×${targetH} (${recommendedScale}%)`;
        } else {
            details += ` · Resolution: ${sourceVideoWidth}×${sourceVideoHeight} (original)`;
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
        formData.append('mode', 'simple');
        formData.append('codec', codecSelectValue);
        
        const percent = parseFloat(outputSizeSliderValue.toString());
        const targetSizeMb = (originalSizeMb! * percent) / 100;
        formData.append('targetSizeMb', targetSizeMb.toFixed(2));
        
        const calculatedScalePercent = calculateOptimalResolution(
            targetSizeMb,
            sourceDuration!,
            sourceVideoWidth!,
            sourceVideoHeight!
        );
        formData.append('scalePercent', calculatedScalePercent.toString());
        
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
        
        console.log('Compression request:', {
            mode: 'simple',
            codec: codecSelectValue,
            targetSizeMb: targetSizeMb.toFixed(2),
            targetPercent: percent,
            sourceDuration: sourceDuration,
            originalSizeMb: originalSizeMb!.toFixed(2),
            scalePercent: calculatedScalePercent
        });
        
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
            } else {
                console.warn('Failed to load video preview');
            }
        } catch (error) {
            console.warn('Failed to load video preview:', error);
        }
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
        codecSelectValue = 'h264';
        updateCodecHelper();
        sourceVideoWidth = null;
        sourceVideoHeight = null;
        sourceDuration = null;
        originalSizeMb = null;
        if (objectUrl) {
            URL.revokeObjectURL(objectUrl);
        }
        objectUrl = null;
    }
</script>

<div class="container">
    <h1>// video-compressor</h1>

    <div 
        class="upload-area" 
        class:dragover={isDragover}
        on:dragover={handleDragOver}
        on:dragleave={handleDragLeave}
        on:drop={handleDrop}
        on:click|self={() => document.getElementById('fileInput')?.click()}
        role="button"
        tabindex="0"
    >
        <input 
            type="file" 
            id="fileInput" 
            accept="video/*" 
            style="display: none;"
            on:change={handleFileInputChange}
        />
        <p style="font-size: 15px;">$ drag & drop video file or click to select</p>
        {#if fileInfo}
            <div class="file-info">→ {fileInfo}</div>
        {/if}
    </div>

    {#if metadataVisible}
        <div id="metadata" class="metadata">
            {@html metadataContent}
        </div>
    {/if}

    {#if controlsVisible}
        <div class="controls" id="controls">
            <div style="margin-bottom: 24px;">
                <label for="outputSizeSlider" style="display: block; margin-bottom: 12px;">
                    <strong>target_output_size</strong>: <span id="outputSizeValue" style="color: #71717a;">{outputSizeValue}</span>
                </label>
                <div style="display: flex; gap: 8px; margin-bottom: 16px; flex-wrap: wrap;">
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
                    style="width: 100%;"
                />
                <div class="helper-text">// drag left to compress more · auto-adjusts quality + resolution</div>
                {#if outputSizeDetails}
                    <div class="estimate-line">→ {outputSizeDetails}</div>
                {/if}
            </div>
            <div class="codec-select">
                <label for="codecSelect" style="display: block; margin-bottom: 8px;"><strong>codec</strong></label>
                <select id="codecSelect" bind:value={codecSelectValue} on:change={() => { updateCodecHelper(); updateOutputSizeDisplay(); }}>
                    <option value="h264">h264 (mp4)</option>
                    <option value="h265">h265 / hevc (mp4)</option>
                    <option value="vp9">vp9 (webm)</option>
                    <option value="av1">av1 (webm)</option>
                </select>
                {#if codecHelperText}
                    <div class="helper-text">// {codecHelperText}</div>
                {/if}
            </div>
        </div>
    {/if}

    {#if !videoPreviewVisible}
        <button 
            id="uploadBtn" 
            disabled={uploadBtnDisabled}
            on:click={handleUpload}
            style="width: 100%; margin-bottom: 24px;"
        >
            $ {uploadBtnText.toLowerCase().replace('&', '+')}
        </button>
    {/if}

    {#if showCancelButton}
        <button 
            id="cancelBtn"
            on:click={handleCancelJob}
            style="width: 100%; margin-bottom: 24px; background-color: #dc2626; border-color: #dc2626;"
        >
            $ cancel compression
        </button>
    {/if}

    {#if progressVisible}
        <div class="progress" id="progress">
            <div class="progress-bar">
                <div class="progress-fill" class:compressing={isCompressing} style="width: {progressPercent}%;"></div>
            </div>
        </div>
    {/if}

    {#if statusVisible}
        <div id="status" class="status-{statusType}">
            → {statusMessage}
        </div>
    {/if}

    {#if videoPreviewVisible}
        <div class="video-preview">
            <h3>// compressed_output</h3>
            <video controls style="width: 100%; border-radius: 8px; border: 1px solid #27272a;">
                {#if videoPreviewUrl}
                    <source src={videoPreviewUrl} type={downloadMimeType || 'video/mp4'}>
                {/if}
                Your browser does not support the video tag.
            </video>
        </div>

        <button id="downloadBtn" on:click={handleDownload} style="width: 100%; margin-bottom: 24px;">
            $ download_compressed_video
        </button>
    {/if}
</div>
