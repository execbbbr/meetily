import React, { useEffect } from 'react';
import { useOnboarding } from '@/contexts/OnboardingContext';
import {
  PermissionsStep,
  CloudSetupStep,
} from './steps';

interface OnboardingFlowProps {
  onComplete: () => void;
}

export function OnboardingFlow({ onComplete }: OnboardingFlowProps) {
  const { currentStep } = useOnboarding();
  const [isMac, setIsMac] = React.useState(false);

  useEffect(() => {
    // Check if running on macOS
    const checkPlatform = async () => {
      try {
        // Dynamic import to avoid SSR issues if any
        const { platform } = await import('@tauri-apps/plugin-os');
        setIsMac(platform() === 'macos');
      } catch (e) {
        console.error('Failed to detect platform:', e);
        // Fallback
        setIsMac(navigator.userAgent.includes('Mac'));
      }
    };
    checkPlatform();
  }, []);

  // 2-Step Onboarding Flow (Cloud Providers — no welcome/download pages):
  // Step 1: Cloud Setup - GitHub Copilot login (summaries) + Azure Speech key (transcription)
  // Step 2: Permissions - Request mic + system audio (macOS only)

  return (
    <div className="onboarding-flow">
      {currentStep === 1 && <CloudSetupStep />}
      {currentStep === 2 && isMac && <PermissionsStep />}
    </div>
  );
}
