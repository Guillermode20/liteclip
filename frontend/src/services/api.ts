import type {
    CompressionStatusResponse,
    FfmpegStatusResponse,
    UserSettingsPayload,
    VersionResponse
} from '../types';
import type { EncoderInfo } from '../types';

const API_BASE = '/api';

type JsonValue = Record<string, unknown> | string | null;

async function parseErrorResponse(response: Response, fallbackMessage: string): Promise<string> {
    try {
        const text = await response.text();
        if (!text) {
            return fallbackMessage;
        }

        try {
            const json = JSON.parse(text) as JsonValue;
            if (typeof json === 'string') {
                return json;
            }

            if (json && typeof json === 'object') {
                const { error, detail, message } = json as Record<string, unknown>;
                return (
                    (typeof error === 'string' && error) ||
                    (typeof detail === 'string' && detail) ||
                    (typeof message === 'string' && message) ||
                    text
                );
            }

            return text;
        } catch {
            return text;
        }
    } catch {
        return fallbackMessage;
    }
}

async function fetchApi<T = void>(
    path: string,
    init: RequestInit = {},
    expectJson: boolean = true,
    fallbackMessage = 'Request failed'
): Promise<T> {
    const response = await fetch(`${API_BASE}${path}`, init);
    if (!response.ok) {
        const message = await parseErrorResponse(response, `${fallbackMessage} (${response.status})`);
        throw new Error(message);
    }
    if (!expectJson) {
        return undefined as T;
    }
    return response.json() as Promise<T>;
}

export async function getSettings(): Promise<UserSettingsPayload> {
    return fetchApi<UserSettingsPayload>('/settings', undefined, true, 'Failed to load settings');
}

export async function getAppVersion(): Promise<VersionResponse> {
    return fetchApi<VersionResponse>('/version', undefined, true, 'Failed to load version');
}

export async function saveSettings(settings: UserSettingsPayload): Promise<UserSettingsPayload> {
    return fetchApi<UserSettingsPayload>(
        '/settings',
        {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(settings)
        },
        true,
        'Failed to save settings'
    );
}

export async function getFfmpegStatus(): Promise<FfmpegStatusResponse> {
    return fetchApi<FfmpegStatusResponse>('/ffmpeg/status', undefined, true, 'FFmpeg status request failed');
}

export async function getFfmpegEncoders(verify = false): Promise<EncoderInfo[]> {
    const query = verify ? '?verify=true' : '';
    return fetchApi<EncoderInfo[]>(
        `/ffmpeg/encoders${query}`,
        undefined,
        true,
        'Failed to fetch encoders'
    );
}

export async function retryFfmpeg(): Promise<void> {
    await fetchApi('/ffmpeg/retry', { method: 'POST' }, false, 'Retry failed');
}

export async function startFfmpeg(): Promise<void> {
    await fetchApi('/ffmpeg/start', { method: 'POST' }, false, 'Start failed');
}

export async function closeApp(): Promise<void> {
    await fetchApi('/app/close', { method: 'POST' }, false, 'Failed to close app');
}

export async function uploadVideo(formData: FormData, signal?: AbortSignal): Promise<{ jobId: string }> {
    return fetchApi<{ jobId: string }>(
        '/compress',
        {
            method: 'POST',
            body: formData,
            signal
        },
        true,
        'Failed to upload video'
    );
}

export async function getJobStatus(jobId: string): Promise<CompressionStatusResponse> {
    return fetchApi<CompressionStatusResponse>(
        `/status/${jobId}`,
        undefined,
        true,
        'Failed to fetch job status'
    );
}

export async function cancelJob(jobId: string): Promise<void> {
    await fetchApi(`/cancel/${jobId}`, { method: 'POST' }, false, 'Failed to cancel job');
}

export async function retryJob(jobId: string): Promise<void> {
    await fetchApi(`/retry/${jobId}`, { method: 'POST' }, false, 'Unable to retry job');
}
