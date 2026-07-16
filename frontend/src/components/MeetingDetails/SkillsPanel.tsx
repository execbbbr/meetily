'use client';

import { useEffect, useState, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import ReactMarkdown from 'react-markdown';
import { toast } from 'sonner';
import { Sparkles, Trash2, ChevronDown, ChevronRight } from 'lucide-react';

interface SkillArtifact {
  id: string;
  meeting_id: string;
  skill_name: string;
  markdown: string;
  created_at: string;
}

interface SkillsPanelProps {
  meetingId: string;
}

/**
 * Shows the skills generated from this meeting, stored separately from the
 * summary so both are visible for the same meeting. Read-only viewer with the
 * ability to remove an artifact. Skills are (re)generated from the /skills page.
 */
export function SkillsPanel({ meetingId }: SkillsPanelProps) {
  const [artifacts, setArtifacts] = useState<SkillArtifact[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [expandedId, setExpandedId] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!meetingId) return;
    setIsLoading(true);
    try {
      const result = await invoke<SkillArtifact[]>('list_skill_artifacts', { meetingId });
      setArtifacts(result);
      // Auto-expand the newest skill for convenience.
      if (result.length > 0) {
        setExpandedId((prev) => prev ?? result[0].id);
      }
    } catch (error) {
      console.error('[SkillsPanel] Failed to load skill artifacts:', error);
    } finally {
      setIsLoading(false);
    }
  }, [meetingId]);

  useEffect(() => {
    load();
  }, [load]);

  const handleDelete = async (id: string) => {
    try {
      await invoke('delete_skill_artifact', { id });
      setArtifacts((prev) => prev.filter((a) => a.id !== id));
      toast.success('Skill removed.');
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      toast.error('Failed to remove skill.', { description: message });
    }
  };

  if (isLoading) {
    return <div className="p-4 text-sm text-gray-500">Loading skills…</div>;
  }

  if (artifacts.length === 0) {
    return (
      <div className="p-4 text-sm text-gray-500">
        <div className="flex items-center gap-2 mb-1">
          <Sparkles className="h-4 w-4 text-gray-400" />
          <span className="font-medium text-gray-700">No skills yet</span>
        </div>
        <p>
          Generate a skill from this meeting on the Skills page. It will appear here
          alongside the summary.
        </p>
      </div>
    );
  }

  return (
    <div className="space-y-2 p-2">
      {artifacts.map((artifact) => {
        const expanded = expandedId === artifact.id;
        return (
          <div key={artifact.id} className="border border-gray-200 rounded-md bg-white">
            <div className="flex items-center justify-between px-3 py-2">
              <button
                className="flex items-center gap-2 flex-1 text-left"
                onClick={() => setExpandedId(expanded ? null : artifact.id)}
              >
                {expanded ? (
                  <ChevronDown className="h-4 w-4 text-gray-500" />
                ) : (
                  <ChevronRight className="h-4 w-4 text-gray-500" />
                )}
                <Sparkles className="h-4 w-4 text-purple-500" />
                <span className="font-medium text-gray-800">{artifact.skill_name}</span>
                <span className="text-xs text-gray-400">
                  {new Date(artifact.created_at).toLocaleString()}
                </span>
              </button>
              <button
                className="text-gray-400 hover:text-red-500 p-1"
                onClick={() => handleDelete(artifact.id)}
                title="Remove skill"
              >
                <Trash2 className="h-4 w-4" />
              </button>
            </div>
            {expanded && (
              <div className="px-4 py-3 border-t border-gray-100 prose prose-sm max-w-none overflow-auto">
                <ReactMarkdown>{artifact.markdown}</ReactMarkdown>
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
