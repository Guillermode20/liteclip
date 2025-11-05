<script>
	let selectedFile = null;
	let jobId = null;
	let statusCheckInterval = null;
	let downloadFileName = null;
	let downloadMimeType = null;
	let objectUrl = null;
	let sourceVideoWidth = null;
	let sourceVideoHeight = null;
	let sourceDuration = null;
	let originalSizeMb = null;
	let statusMessage = '';
	let statusType = '';
	let showStatus = false;
	let progress = 0;
	let showProgress = false;
	let showDownloadBtn = false;
	let showControls = false;
	let showMetadata = false;
	let uploadBtnDisabled = true;
	let uploadBtnText = 'Upload & Compress Video';
	let outputSizePercent = 100;
	let codec = 'h264';
	let outputSizeSliderDisabled = true;
	let outputSizeValue = '--';
	let outputSizeDetails = '';
	let isDragging = false;

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

	function formatFileSize(bytes) {
		if (bytes === 0) return '0 Bytes';
		const k = 1024;
		const sizes = ['Bytes', 'KB', 'MB', 'GB'];
		const i = Math.floor(Math.log(bytes) / Math.log(k));
		return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
	}

	function handleFileSelect(file) {
		if (!file.type.startsWith('video/')) {
			alert('Please select a video file');
			return;
		}

		selectedFile = file;
		originalSizeMb = selectedFile.size / (1024 * 1024);
		uploadBtnDisabled = false;
		uploadBtnText = 'Upload & Compress Video';
		showControls = true;
		showMetadata = false;

		outputSizeSliderDisabled = true;
		outputSizeValue = '--';
		outputSizeDetails = 'Reading video metadata...';

		// Load metadata
		if (objectUrl) URL.revokeObjectURL(objectUrl);
		objectUrl = URL.createObjectURL(file);
		const videoEl = document.createElement('video');
		videoEl.preload = 'metadata';
		videoEl.src = objectUrl;
		videoEl.addEventListener('loadedmetadata', () => {
			sourceVideoWidth = videoEl.videoWidth || null;
			sourceVideoHeight = videoEl.videoHeight || null;
			const duration = isFinite(videoEl.duration) ? videoEl.duration : null;
			sourceDuration = duration;
			
			showMetadata = true;
			
			// Configure slider
			outputSizePercent = 100;
			outputSizeSliderDisabled = false;
			updateOutputSizeDisplay();
		}, { once: true });
	}

	function handleDragOver(e) {
		e.preventDefault();
		isDragging = true;
	}

	function handleDragLeave() {
		isDragging = false;
	}

	function handleDrop(e) {
		e.preventDefault();
		isDragging = false;
		const files = e.dataTransfer.files;
		if (files.length > 0) {
			handleFileSelect(files[0]);
		}
	}

	function handleFileInputChange(e) {
		if (e.target.files.length > 0) {
			handleFileSelect(e.target.files[0]);
		}
	}

	function calculateOptimalResolution(targetSizeMb, durationSec, width, height) {
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

		const percent = parseFloat(outputSizePercent);
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

	function handlePresetClick(targetPercent) {
		if (outputSizeSliderDisabled) return;
		outputSizePercent = targetPercent;
		updateOutputSizeDisplay();
	}

	async function handleUpload() {
		if (!selectedFile) return;

		uploadBtnDisabled = true;
		uploadBtnText = 'Uploading...';
		showProgress = true;
		progress = 10;

		const formData = new FormData();
		formData.append('file', selectedFile);
		formData.append('mode', 'simple');
		formData.append('codec', codec);
		
		const percent = parseFloat(outputSizePercent);
		const targetSizeMb = (originalSizeMb * percent) / 100;
		formData.append('targetSizeMb', targetSizeMb.toFixed(2));
		
		const calculatedScalePercent = calculateOptimalResolution(
			targetSizeMb,
			sourceDuration,
			sourceVideoWidth,
			sourceVideoHeight
		);
		formData.append('scalePercent', calculatedScalePercent);
		
		if (sourceDuration && isFinite(sourceDuration)) {
			formData.append('sourceDuration', sourceDuration.toFixed(3));
		}
		if (Number.isFinite(sourceVideoWidth) && sourceVideoWidth > 0) {
			formData.append('sourceWidth', sourceVideoWidth);
		}
		if (Number.isFinite(sourceVideoHeight) && sourceVideoHeight > 0) {
			formData.append('sourceHeight', sourceVideoHeight);
		}
		formData.append('originalSizeBytes', selectedFile.size);

		try {
			const response = await fetch('/api/compress', {
				method: 'POST',
				body: formData
			});

			if (!response.ok) {
				let errorMsg = `Server error (${response.status})`;
				try {
					const errorData = await response.json();
					errorMsg = errorData.error || errorData.detail || errorMsg;
				} catch (e) {
					errorMsg = await response.text() || errorMsg;
				}
				throw new Error(errorMsg);
			}

			const result = await response.json();
			jobId = result.jobId;

			progress = 100;
			showStatusMessage('Video uploaded successfully. Compressing...', 'processing');
			showStatus = true;

			// Start checking status
			statusCheckInterval = setInterval(checkStatus, 2000);

		} catch (error) {
			console.error('Upload failed:', error);
			let errorMessage = error.message;
			
			if (errorMessage.includes('NetworkError') || errorMessage.includes('Failed to fetch')) {
				errorMessage = 'Network error: File may be too large or server is unreachable. Please try a smaller file or check your connection.';
			} else if (errorMessage.includes('413')) {
				errorMessage = 'File is too large. The server cannot accept files this big.';
			}
			
			showStatusMessage('Upload failed: ' + errorMessage, 'error');
			showStatus = true;
			uploadBtnDisabled = false;
			uploadBtnText = 'Upload & Compress Video';
			showProgress = false;
		}
	}

	async function checkStatus() {
		try {
			const response = await fetch(`/api/status/${jobId}`);
			if (response.ok) {
				const result = await response.json();
				if (result.status === 'processing') {
					const progressPercent = Math.max(10, Math.min(95, result.progress || 0));
					progress = progressPercent;
					showStatusMessage(`Compressing video... ${progressPercent.toFixed(1)}%`, 'processing');
					showStatus = true;
				} else if (result.status === 'completed') {
					clearInterval(statusCheckInterval);
					progress = 100;
					downloadFileName = result.outputFilename || `compressed_${selectedFile.name}`;
					downloadMimeType = result.outputMimeType || 'video/mp4';
					showStatusMessage('Compression complete! Click download button.', 'success');
					showStatus = true;
					showDownloadBtn = true;
					showProgress = false;
				} else if (result.status === 'failed') {
					clearInterval(statusCheckInterval);
					showStatusMessage('Compression failed: ' + (result.message || 'Unknown error'), 'error');
					showStatus = true;
					uploadBtnDisabled = false;
					uploadBtnText = 'Upload & Compress Video';
					showProgress = false;
				}
			} else {
				clearInterval(statusCheckInterval);
				const errorData = await response.json().catch(() => ({}));
				showStatusMessage('Compression failed: ' + (errorData.error || 'Unknown error'), 'error');
				showStatus = true;
				uploadBtnDisabled = false;
				uploadBtnText = 'Upload & Compress Video';
				showProgress = false;
			}
		} catch (error) {
			console.error('Status check failed:', error);
			clearInterval(statusCheckInterval);
			showStatusMessage('Failed to check status: ' + error.message, 'error');
			showStatus = true;
			uploadBtnDisabled = false;
			uploadBtnText = 'Upload & Compress Video';
			showProgress = false;
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

	function showStatusMessage(message, type) {
		statusMessage = message;
		statusType = type;
		showStatus = true;
	}

	function resetInterface() {
		if (statusCheckInterval) {
			clearInterval(statusCheckInterval);
			statusCheckInterval = null;
		}
		selectedFile = null;
		jobId = null;
		showStatus = false;
		showDownloadBtn = false;
		showProgress = false;
		progress = 0;
		uploadBtnDisabled = true;
		uploadBtnText = 'Upload & Compress Video';
		showControls = false;
		showMetadata = false;
		downloadFileName = null;
		downloadMimeType = null;
		outputSizeSliderDisabled = true;
		outputSizePercent = 100;
		outputSizeValue = '--';
		outputSizeDetails = '';
		codec = 'h264';
		sourceVideoWidth = null;
		sourceVideoHeight = null;
		sourceDuration = null;
		originalSizeMb = null;
		if (objectUrl) URL.revokeObjectURL(objectUrl);
		objectUrl = null;
	}

	function handleOutputSizeChange() {
		updateOutputSizeDisplay();
	}

	function handleCodecChange() {
		updateOutputSizeDisplay();
	}

	function getVideoMetadata() {
		if (!selectedFile || !showMetadata) return '';
		
		const kbps = sourceDuration ? Math.round((selectedFile.size * 8) / sourceDuration / 1000) : null;
		const dimsText = (sourceVideoWidth && sourceVideoHeight) ? `${sourceVideoWidth}×${sourceVideoHeight}` : 'Unknown';
		const durationText = sourceDuration ? `${sourceDuration.toFixed(2)}s` : 'Unknown';
		const bitrateText = kbps ? `${kbps} kbps (approx)` : 'Unknown';
		
		return {
			size: formatFileSize(selectedFile.size),
			type: selectedFile.type || 'Unknown',
			duration: durationText,
			resolution: dimsText,
			bitrate: bitrateText
		};
	}

	$: metadata = getVideoMetadata();
	$: codecHelper = codecDetails[codec]?.helper || '';
</script>

<svelte:window on:beforeunload={() => {
	if (statusCheckInterval) {
		clearInterval(statusCheckInterval);
	}
	if (objectUrl) {
		URL.revokeObjectURL(objectUrl);
	}
}} />

<div class="container">
	<h1>Video Compressor</h1>

	<div 
		class="upload-area" 
		class:dragover={isDragging}
		on:click={() => {
			const fileInput = document.getElementById('fileInput');
			if (fileInput) fileInput.click();
		}}
		on:dragover={handleDragOver}
		on:dragleave={handleDragLeave}
		on:drop={handleDrop}
		role="button"
		tabindex="0"
	>
		<input type="file" id="fileInput" accept="video/*" style="display: none;" on:change={handleFileInputChange} />
		<p>Drag and drop a video file here, or click to select</p>
		{#if selectedFile}
			<div class="file-info">Selected: {selectedFile.name} ({formatFileSize(selectedFile.size)})</div>
		{/if}
	</div>

	{#if showMetadata && metadata}
		<div class="metadata">
			<div><strong>File size</strong>: {metadata.size}</div>
			<div><strong>Type</strong>: {metadata.type}</div>
			<div><strong>Duration</strong>: {metadata.duration}</div>
			<div><strong>Resolution</strong>: {metadata.resolution}</div>
			<div><strong>Bitrate</strong>: {metadata.bitrate}</div>
		</div>
	{/if}

	{#if showControls}
		<div class="controls">
			<div class="control-group">
				<label for="outputSizeSlider">
					<strong>Target output size</strong>: <span>{outputSizeValue}</span>
				</label>
				<div class="preset-buttons">
					<button type="button" class="preset-btn" on:click={() => handlePresetClick(25)} disabled={outputSizeSliderDisabled}>
						75% smaller
					</button>
					<button type="button" class="preset-btn" on:click={() => handlePresetClick(50)} disabled={outputSizeSliderDisabled}>
						50% smaller
					</button>
					<button type="button" class="preset-btn" on:click={() => handlePresetClick(75)} disabled={outputSizeSliderDisabled}>
						25% smaller
					</button>
				</div>
				<input 
					type="range" 
					id="outputSizeSlider" 
					min="1" 
					max="100" 
					step="0.5" 
					bind:value={outputSizePercent}
					disabled={outputSizeSliderDisabled}
					on:input={handleOutputSizeChange}
				/>
				<div class="helper-text">Drag left to compress more. Automatically adjusts quality and resolution.</div>
				<div class="estimate-line">{outputSizeDetails}</div>
			</div>
			<div class="codec-select">
				<label for="codecSelect"><strong>Codec</strong></label>
				<select id="codecSelect" bind:value={codec} on:change={handleCodecChange}>
					<option value="h264">H.264 (MP4)</option>
					<option value="h265">H.265 / HEVC (MP4)</option>
					<option value="vp9">VP9 (WebM)</option>
					<option value="av1">AV1 (WebM)</option>
				</select>
				<div class="helper-text">{codecHelper}</div>
			</div>
		</div>
	{/if}

	<button 
		id="uploadBtn" 
		disabled={uploadBtnDisabled} 
		on:click={handleUpload}
		on:click|stopPropagation
	>
		{uploadBtnText}
	</button>

	{#if showProgress}
		<div class="progress">
			<div class="progress-bar">
				<div class="progress-fill" class:compressing={progress < 100} style="width: {progress}%"></div>
			</div>
		</div>
	{/if}

	{#if showStatus}
		<div class="status status-{statusType}">
			{statusMessage}
		</div>
	{/if}

	{#if showDownloadBtn}
		<button id="downloadBtn" on:click={handleDownload}>
			Download Compressed Video
		</button>
	{/if}
</div>

<style>
	:global(*) {
		font-family: 'Consolas', 'Monaco', 'Courier New', monospace;
	}
	
	:global(body) {
		font-family: 'Consolas', 'Monaco', 'Courier New', monospace;
		max-width: 800px;
		margin: 0 auto;
		padding: 20px;
		background-color: #1e1e1e;
		color: #d4d4d4;
	}

	.container {
		background-color: #252526;
		padding: 30px;
		border-radius: 8px;
		box-shadow: 0 2px 10px rgba(0,0,0,0.5);
		border: 1px solid #3e3e42;
	}

	h1 {
		color: #569cd6;
		text-align: center;
		margin-bottom: 30px;
	}

	.upload-area {
		border: 2px dashed #3e3e42;
		border-radius: 8px;
		padding: 40px;
		text-align: center;
		margin-bottom: 20px;
		transition: border-color 0.3s;
		background-color: #1e1e1e;
		color: #d4d4d4;
		cursor: pointer;
	}

	.upload-area:hover {
		border-color: #569cd6;
	}

	.upload-area.dragover {
		border-color: #569cd6;
		background-color: #2d2d30;
	}

	.file-info {
		margin-top: 10px;
		font-size: 14px;
		color: #858585;
	}

	.metadata {
		margin-top: 12px;
		margin-bottom: 16px;
		text-align: left;
		font-size: 14px;
		color: #d4d4d4;
	}

	.metadata strong {
		color: #569cd6;
	}

	.controls {
		margin-bottom: 20px;
		text-align: left;
	}

	.control-group {
		margin-bottom: 16px;
	}

	.preset-buttons {
		display: flex;
		gap: 8px;
		margin-top: 8px;
		margin-bottom: 8px;
		flex-wrap: wrap;
	}

	.preset-btn {
		padding: 6px 12px;
		border: 1px solid #569cd6;
		background-color: #252526;
		color: #569cd6;
		border-radius: 4px;
		cursor: pointer;
		font-size: 13px;
		transition: all 0.2s;
		font-family: 'Consolas', 'Monaco', 'Courier New', monospace;
	}

	.preset-btn:hover:not(:disabled) {
		background-color: #569cd6;
		color: #1e1e1e;
	}

	.preset-btn:active:not(:disabled) {
		transform: scale(0.95);
	}

	.preset-btn:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	input[type="range"] {
		width: 100%;
		background-color: #1e1e1e;
		-webkit-appearance: none;
		appearance: none;
		height: 8px;
		border-radius: 4px;
		outline: none;
	}

	input[type="range"]::-webkit-slider-runnable-track {
		background: #3e3e42;
		height: 8px;
		border-radius: 4px;
	}

	input[type="range"]::-webkit-slider-thumb {
		-webkit-appearance: none;
		appearance: none;
		width: 18px;
		height: 18px;
		border-radius: 50%;
		background-color: #569cd6;
		cursor: pointer;
		margin-top: -5px;
	}

	input[type="range"]::-moz-range-track {
		background: #3e3e42;
		height: 8px;
		border-radius: 4px;
	}

	input[type="range"]::-moz-range-thumb {
		width: 18px;
		height: 18px;
		border-radius: 50%;
		background-color: #569cd6;
		cursor: pointer;
		border: none;
	}

	input[type="range"]:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.codec-select {
		margin-bottom: 12px;
	}

	.codec-select select {
		width: 100%;
		padding: 8px;
		border-radius: 4px;
		border: 1px solid #3e3e42;
		font-size: 14px;
		background-color: #1e1e1e;
		color: #d4d4d4;
		font-family: 'Consolas', 'Monaco', 'Courier New', monospace;
	}

	.codec-select select option {
		background-color: #252526;
		color: #d4d4d4;
	}

	.helper-text {
		font-size: 12px;
		color: #858585;
		margin-top: 4px;
	}

	.estimate-line {
		font-size: 12px;
		color: #d4d4d4;
		margin-top: 4px;
	}

	label strong {
		color: #569cd6;
	}

	#uploadBtn {
		width: 100%;
		background-color: #0e639c;
		color: #ffffff;
		border: none;
		padding: 12px 24px;
		border-radius: 4px;
		cursor: pointer;
		font-size: 16px;
		margin-bottom: 20px;
		font-family: 'Consolas', 'Monaco', 'Courier New', monospace;
	}

	#uploadBtn:hover:not(:disabled) {
		background-color: #1177bb;
	}

	#uploadBtn:disabled {
		background-color: #3e3e42;
		color: #858585;
		cursor: not-allowed;
	}

	.progress {
		margin-top: 10px;
		margin-bottom: 20px;
	}

	.progress-bar {
		width: 100%;
		height: 20px;
		background-color: #1e1e1e;
		border-radius: 10px;
		overflow: hidden;
		border: 1px solid #3e3e42;
	}

	.progress-fill {
		height: 100%;
		background-color: #569cd6;
		transition: width 0.3s;
	}

	.progress-fill.compressing {
		background: linear-gradient(90deg, #569cd6 0%, #0e639c 50%, #569cd6 100%);
		background-size: 200% 100%;
		animation: pulse 2s ease-in-out infinite;
	}

	@keyframes pulse {
		0%, 100% { background-position: 0% 0%; }
		50% { background-position: 100% 0%; }
	}

	.status {
		margin-top: 20px;
		padding: 10px;
		border-radius: 4px;
	}

	.status-processing {
		background-color: #4a4a2a;
		color: #dcdcaa;
		border: 1px solid #6a6a3a;
	}

	.status-success {
		background-color: #2d4a2d;
		color: #9cdcfe;
		border: 1px solid #4d6a4d;
	}

	.status-error {
		background-color: #4a2d2d;
		color: #f48771;
		border: 1px solid #6a4d4d;
	}

	#downloadBtn {
		width: 100%;
		background-color: #0e7c0e;
		color: #ffffff;
		border: none;
		padding: 12px 24px;
		border-radius: 4px;
		cursor: pointer;
		font-size: 16px;
		margin-top: 10px;
		font-family: 'Consolas', 'Monaco', 'Courier New', monospace;
	}

	#downloadBtn:hover {
		background-color: #117711;
	}
</style>

