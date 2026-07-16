import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Cloud, Github, Key, Check } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Label } from '@/components/ui/label';
import { OnboardingContainer } from '../OnboardingContainer';
import { useOnboarding } from '@/contexts/OnboardingContext';
import { CopilotSettings } from '@/components/CopilotSettings';
import { toast } from 'sonner';

/**
 * Cloud setup step: instead of downloading local transcription/summary models,
 * the user configures cloud providers:
 *   - Summarization: GitHub Copilot (device-flow login, reuses CopilotSettings)
 *   - Transcription: Azure Speech (key + region)
 *
 * This replaces the old DownloadProgressStep so no local model is ever fetched.
 */
export function CloudSetupStep() {
  const { completeOnboarding } = useOnboarding();

  const [isMac, setIsMac] = useState(false);
  const [azureKey, setAzureKey] = useState('');
  const [azureRegion, setAzureRegion] = useState('centralus');
  const [azureSaved, setAzureSaved] = useState(false);
  const [isSavingAzure, setIsSavingAzure] = useState(false);
  const [isCompleting, setIsCompleting] = useState(false);

  useEffect(() => {
    const checkPlatform = async () => {
      try {
        const { platform } = await import('@tauri-apps/plugin-os');
        setIsMac(platform() === 'macos');
      } catch {
        setIsMac(navigator.userAgent.includes('Mac'));
      }
    };
    checkPlatform();
  }, []);

  const handleSaveAzure = async () => {
    if (!azureKey.trim()) {
      toast.error('Please enter your Azure Speech key.');
      return;
    }
    setIsSavingAzure(true);
    try {
      // Persist Azure Speech credentials on the transcript config. Provider stays
      // as-is; the key/region are what the Azure transcription path consumes.
      await invoke('api_save_transcript_config', {
        provider: 'azure',
        model: 'azure-realtime',
        apiKey: null,
        diarizationEnabled: true,
        diarizationProvider: 'azure',
        azureSpeechKey: azureKey.trim(),
        azureSpeechRegion: azureRegion.trim() || 'centralus',
      });
      setAzureSaved(true);
      toast.success('Azure Speech configured.');
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      toast.error('Failed to save Azure configuration.', { description: message });
    } finally {
      setIsSavingAzure(false);
    }
  };

  const handleFinish = async () => {
    setIsCompleting(true);
    try {
      await completeOnboarding();
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      toast.error('Failed to complete setup.', { description: message });
      setIsCompleting(false);
    }
  };

  return (
    <OnboardingContainer
      title="Connect Cloud Providers"
      description="Meetily can run fully on cloud AI — no local models to download. Sign in to GitHub Copilot for summaries and add your Azure Speech key for transcription."
      step={3}
      totalSteps={isMac ? 4 : 3}
    >
      <div className="flex flex-col items-center space-y-6 w-full">
        {/* Summarization: GitHub Copilot */}
        <div className="w-full max-w-md bg-white rounded-lg border border-gray-200 p-5">
          <div className="flex items-center gap-2 mb-3">
            <Github className="w-5 h-5 text-gray-800" />
            <h3 className="font-semibold text-gray-900">Summarization — GitHub Copilot</h3>
          </div>
          <p className="text-sm text-gray-600 mb-4">
            Sign in with GitHub to use Copilot models for meeting summaries and skills.
          </p>
          <CopilotSettings />
        </div>

        {/* Transcription: Azure Speech */}
        <div className="w-full max-w-md bg-white rounded-lg border border-gray-200 p-5">
          <div className="flex items-center gap-2 mb-3">
            <Cloud className="w-5 h-5 text-blue-600" />
            <h3 className="font-semibold text-gray-900">Transcription — Azure Speech</h3>
            {azureSaved && <Check className="w-4 h-4 text-green-600" />}
          </div>
          <p className="text-sm text-gray-600 mb-4">
            Enter your Azure Speech key and region to transcribe meetings in the cloud.
          </p>
          <div className="space-y-3">
            <div>
              <Label htmlFor="azure-key" className="text-sm">Azure Speech Key</Label>
              <input
                id="azure-key"
                type="password"
                value={azureKey}
                onChange={(e) => { setAzureKey(e.target.value); setAzureSaved(false); }}
                placeholder="Paste your Azure Speech key"
                className="w-full mt-1 px-3 py-2 border border-gray-300 rounded-md text-sm"
              />
            </div>
            <div>
              <Label htmlFor="azure-region" className="text-sm">Region</Label>
              <input
                id="azure-region"
                type="text"
                value={azureRegion}
                onChange={(e) => { setAzureRegion(e.target.value); setAzureSaved(false); }}
                placeholder="e.g. centralus"
                className="w-full mt-1 px-3 py-2 border border-gray-300 rounded-md text-sm"
              />
            </div>
            <Button
              onClick={handleSaveAzure}
              disabled={isSavingAzure || !azureKey.trim()}
              variant="outline"
              className="w-full"
            >
              <Key className="w-4 h-4 mr-2" />
              {isSavingAzure ? 'Saving…' : azureSaved ? 'Saved' : 'Save Azure Key'}
            </Button>
          </div>
        </div>

        {/* Finish — cloud setup is optional; user can configure later in Settings */}
        <div className="w-full max-w-md">
          <Button
            onClick={handleFinish}
            disabled={isCompleting}
            className="w-full h-11 bg-gray-900 hover:bg-gray-800 text-white"
          >
            {isCompleting ? 'Finishing…' : 'Continue'}
          </Button>
          <p className="text-xs text-gray-400 text-center mt-2">
            You can change or add providers anytime in Settings.
          </p>
        </div>
      </div>
    </OnboardingContainer>
  );
}
