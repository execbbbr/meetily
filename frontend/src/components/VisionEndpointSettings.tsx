'use client';

import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { toast } from 'sonner';

interface VisionEndpointConfig {
  endpoint: string;
  apiKey: string | null;
  model: string;
}

export function VisionEndpointSettings() {
  const [endpoint, setEndpoint] = useState('');
  const [apiKey, setApiKey] = useState('');
  const [model, setModel] = useState('');
  const [isSaving, setIsSaving] = useState(false);

  useEffect(() => {
    const load = async () => {
      try {
        const config = (await invoke('api_get_skill_vision_config')) as VisionEndpointConfig | null;
        if (!config) return;

        setEndpoint(config.endpoint ?? '');
        setApiKey(config.apiKey ?? '');
        setModel(config.model ?? '');
      } catch (error) {
        console.error('Failed to load vision endpoint config:', error);
      }
    };

    load();
  }, []);

  const handleSave = async () => {
    if (!endpoint.trim() || !model.trim()) {
      toast.error('Endpoint and model are required.');
      return;
    }

    setIsSaving(true);
    try {
      await invoke('api_save_skill_vision_config', {
        endpoint: endpoint.trim(),
        apiKey: apiKey.trim() || null,
        model: model.trim(),
      });

      toast.success('Vision endpoint settings saved.');
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      toast.error('Failed to save vision endpoint settings.', { description: message });
    } finally {
      setIsSaving(false);
    }
  };

  return (
    <div className="bg-white rounded-lg border border-gray-200 p-6 shadow-sm space-y-4">
      <div>
        <h3 className="text-lg font-semibold mb-1">Vision Endpoint (Skill Visual Generation)</h3>
        <p className="text-sm text-gray-600">
          Configure an OpenAI-compatible endpoint with vision support for visual timeline skill generation.
        </p>
      </div>

      <div>
        <Label htmlFor="vision-endpoint">Base URL</Label>
        <Input
          id="vision-endpoint"
          value={endpoint}
          onChange={(e) => setEndpoint(e.target.value)}
          placeholder="https://your-openai-compatible-endpoint/v1"
          className="mt-1"
        />
      </div>

      <div>
        <Label htmlFor="vision-api-key">API Key</Label>
        <Input
          id="vision-api-key"
          type="password"
          value={apiKey}
          onChange={(e) => setApiKey(e.target.value)}
          placeholder="sk-..."
          className="mt-1"
        />
      </div>

      <div>
        <Label htmlFor="vision-model">Model</Label>
        <Input
          id="vision-model"
          value={model}
          onChange={(e) => setModel(e.target.value)}
          placeholder="gpt-4o"
          className="mt-1"
        />
      </div>

      <div className="flex justify-end">
        <Button onClick={handleSave} disabled={isSaving}>
          {isSaving ? 'Saving...' : 'Save Vision Endpoint'}
        </Button>
      </div>
    </div>
  );
}
