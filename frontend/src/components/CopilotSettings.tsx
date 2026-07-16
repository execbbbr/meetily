'use client';

import { useEffect, useState, useRef, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';
import { Label } from '@/components/ui/label';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { toast } from 'sonner';

interface CopilotStatus {
  logged_in: boolean;
  base_url: string | null;
}

interface CopilotLoginStart {
  device_code: string;
  user_code: string;
  verification_uri: string;
  interval: number;
  expires_in: number;
}

interface CopilotPollResult {
  status: 'complete' | 'pending' | 'slow_down';
  interval?: number | null;
}

interface CopilotModel {
  id: string;
  name: string;
}

const COPILOT_MODEL_STORAGE_KEY = 'meetily.copilotModel';

export function CopilotSettings() {
  const [loggedIn, setLoggedIn] = useState(false);
  const [loading, setLoading] = useState(true);
  const [loginInProgress, setLoginInProgress] = useState(false);
  const [userCode, setUserCode] = useState<string | null>(null);
  const [verificationUri, setVerificationUri] = useState<string | null>(null);
  const [models, setModels] = useState<CopilotModel[]>([]);
  const [selectedModel, setSelectedModel] = useState<string>('');

  const pollTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const cancelled = useRef(false);

  const clearPoll = () => {
    if (pollTimer.current) {
      clearTimeout(pollTimer.current);
      pollTimer.current = null;
    }
  };

  const loadModels = useCallback(async () => {
    try {
      const list = (await invoke('copilot_list_models')) as CopilotModel[];
      setModels(list);
      // Restore previously chosen model if still available.
      const saved = localStorage.getItem(COPILOT_MODEL_STORAGE_KEY) ?? '';
      if (saved && list.some((m) => m.id === saved)) {
        setSelectedModel(saved);
      } else if (list.length > 0) {
        setSelectedModel(list[0].id);
      }
    } catch (error) {
      console.error('Failed to list Copilot models:', error);
      toast.error('Failed to load Copilot models', {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  }, []);

  const refreshStatus = useCallback(async () => {
    try {
      const status = (await invoke('copilot_status')) as CopilotStatus;
      setLoggedIn(status.logged_in);
      if (status.logged_in) {
        await loadModels();
      }
    } catch (error) {
      console.error('Failed to get Copilot status:', error);
    } finally {
      setLoading(false);
    }
  }, [loadModels]);

  useEffect(() => {
    cancelled.current = false;
    refreshStatus();
    return () => {
      cancelled.current = true;
      clearPoll();
    };
  }, [refreshStatus]);

  const pollOnce = useCallback(
    async (deviceCode: string, intervalMs: number, expiresAtMs: number) => {
      if (cancelled.current) return;
      if (Date.now() > expiresAtMs) {
        setLoginInProgress(false);
        setUserCode(null);
        setVerificationUri(null);
        toast.error('GitHub login timed out. Please try again.');
        return;
      }

      try {
        const result = (await invoke('copilot_poll_login', {
          deviceCode,
        })) as CopilotPollResult;

        if (result.status === 'complete') {
          setLoginInProgress(false);
          setUserCode(null);
          setVerificationUri(null);
          setLoggedIn(true);
          toast.success('Signed in to GitHub Copilot');
          await loadModels();
          return;
        }

        // pending or slow_down: keep polling (respect a longer interval on slow_down).
        const nextInterval =
          result.status === 'slow_down' && result.interval
            ? result.interval * 1000
            : intervalMs;
        pollTimer.current = setTimeout(
          () => pollOnce(deviceCode, nextInterval, expiresAtMs),
          nextInterval,
        );
      } catch (error) {
        setLoginInProgress(false);
        setUserCode(null);
        setVerificationUri(null);
        toast.error('GitHub login failed', {
          description: error instanceof Error ? error.message : String(error),
        });
      }
    },
    [loadModels],
  );

  const handleLogin = async () => {
    setLoginInProgress(true);
    try {
      const start = (await invoke('copilot_start_login')) as CopilotLoginStart;
      setUserCode(start.user_code);
      setVerificationUri(start.verification_uri);

      const intervalMs = Math.max(start.interval, 1) * 1000;
      const expiresAtMs = Date.now() + start.expires_in * 1000;
      pollTimer.current = setTimeout(
        () => pollOnce(start.device_code, intervalMs, expiresAtMs),
        intervalMs,
      );
    } catch (error) {
      setLoginInProgress(false);
      toast.error('Failed to start GitHub login', {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  };

  const handleLogout = async () => {
    clearPoll();
    try {
      await invoke('copilot_logout');
      setLoggedIn(false);
      setModels([]);
      setSelectedModel('');
      setUserCode(null);
      setVerificationUri(null);
      setLoginInProgress(false);
      toast.success('Signed out of GitHub Copilot');
    } catch (error) {
      toast.error('Failed to sign out', {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  };

  const handleSelectModel = (modelId: string) => {
    setSelectedModel(modelId);
    localStorage.setItem(COPILOT_MODEL_STORAGE_KEY, modelId);
    toast.success(`Copilot model set to ${modelId}`);
  };

  return (
    <div className="bg-white rounded-lg border border-gray-200 p-6 shadow-sm space-y-4">
      <div>
        <h3 className="text-lg font-semibold mb-1">GitHub Copilot</h3>
        <p className="text-sm text-gray-600">
          Sign in with your GitHub Copilot subscription to use its models (GPT-4o,
          Claude, and more) for summaries and skill generation. Requires an active
          Copilot subscription.
        </p>
      </div>

      {loading ? (
        <p className="text-sm text-gray-500">Checking Copilot status…</p>
      ) : loggedIn ? (
        <>
          <div className="flex items-center gap-2">
            <span className="inline-block h-2 w-2 rounded-full bg-green-500" />
            <span className="text-sm text-gray-700">Signed in</span>
          </div>

          <div>
            <Label htmlFor="copilot-model">Model</Label>
            <Select value={selectedModel} onValueChange={handleSelectModel}>
              <SelectTrigger id="copilot-model" className="mt-1">
                <SelectValue placeholder="Select a Copilot model" />
              </SelectTrigger>
              <SelectContent>
                {models.map((m) => (
                  <SelectItem key={m.id} value={m.id}>
                    {m.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {models.length === 0 && (
              <p className="text-xs text-gray-500 mt-1">
                No models available. If a model you expect is missing, enable it in
                VS Code Copilot Chat first.
              </p>
            )}
          </div>

          <div className="flex justify-end">
            <Button variant="outline" onClick={handleLogout}>
              Sign out
            </Button>
          </div>
        </>
      ) : loginInProgress && userCode ? (
        <div className="space-y-3">
          <p className="text-sm text-gray-700">
            A browser window was opened. Enter this code on GitHub to authorize:
          </p>
          <div className="flex items-center gap-3">
            <code className="px-3 py-2 rounded bg-gray-100 text-lg font-mono tracking-widest">
              {userCode}
            </code>
            {verificationUri && (
              <a
                href={verificationUri}
                target="_blank"
                rel="noopener noreferrer"
                className="text-sm text-blue-600 underline"
              >
                Open GitHub
              </a>
            )}
          </div>
          <p className="text-xs text-gray-500">Waiting for authorization…</p>
        </div>
      ) : (
        <div className="flex justify-end">
          <Button onClick={handleLogin} disabled={loginInProgress}>
            {loginInProgress ? 'Starting…' : 'Sign in with GitHub'}
          </Button>
        </div>
      )}
    </div>
  );
}
