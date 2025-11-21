import type {
    CompressionStatusResponse,
    FfmpegStatusResponse,
    UserSettingsPayload
} from '../types';

const API_BASE = '/api';

export async function getSettings(): Promise<UserSettingsPayload> {
    const response = await fetch(`${API_BASE}/settings`);
    if (!response.ok) throw new Error(`Failed to load settings: ${response.status}`);
    return response.json();
}

export async function saveSettings(settings: UserSettingsPayload): Promise<UserSettingsPayload> {
    const response = await fetch(`${API_BASE}/settings`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(settings)
    });
    if (!response.ok) {
        const text = await response.text();
        throw new Error(text || 'Failed to save settings');
    }
    return response.json();
}

export async function getFfmpegStatus(): Promise<FfmpegStatusResponse> {
    const response = await fetch(`${API_BASE}/ffmpeg/status`);
    if (!response.ok) throw new Error(`FFmpeg status request failed (${response.status})`);
    return response.json();
}

export async function retryFfmpeg(): Promise<void> {
    const response = await fetch(`${API_BASE}/ffmpeg/retry`, { method: 'POST' });
    if (!response.ok) {
        const text = await response.text();
        throw new Error(text || `Retry failed (${response.status})`);
    }
}

export async function uploadVideo(formData: FormData): Promise<{ jobId: string }> {
    const response = await fetch(`${API_BASE}/compress`, {
        method: 'POST',
        body: formData
    });
    if (!response.ok) {
        let errorMsg = `Server error (${response.status})`;
        try {
            const text = await response.text();
            try {
                const data = JSON.parse(text);
                errorMsg = data.error || data.detail || errorMsg;
            } catch {
                errorMsg = text || errorMsg;
            }
        } catch {
            // ignore
        }
        throw new Error(errorMsg);
    }
    return response.json();
}

export async function getJobStatus(jobId: string): Promise<CompressionStatusResponse> {
    const response = await fetch(`${API_BASE}/status/${jobId}`);
    if (!response.ok) {
         let errorMessage = 'Unknown error';
         try {
             const text = await response.text();
             try {
                 const data = JSON.parse(text);
                 errorMessage = data.error || errorMessage;
             } catch {
                 errorMessage = text || errorMessage;
             }
         } catch {
             // ignore
         }
         throw new Error(errorMessage);
    }
    return response.json();
}

export async function cancelJob(jobId: string): Promise<void> {
    const response = await fetch(`${API_BASE}/cancel/${jobId}`, { method: 'POST' });
    if (!response.ok) {
        const error = await response.json();
        throw new Error(error.error || 'Unknown error');
    }
}

export async function retryJob(jobId: string): Promise<void> {
    const response = await fetch(`${API_BASE}/retry/${jobId}`, { method: 'POST' });
    if (!response.ok) {
        let errorMessage = 'Unable to retry job';
        try {
            const data = await response.json();
            errorMessage = data.error || errorMessage;
        } catch {
             const text = await response.text();
             errorMessage = text || errorMessage;
        }
        throw new Error(errorMessage);
    }
}
