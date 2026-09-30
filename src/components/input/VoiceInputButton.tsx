/**
 * VoiceInputButton — 语音输入按钮（会话输入栏与群聊输入栏共用）
 *
 * 录音 → 转写 → 把文本通过 `onTranscribed` 交回调用方（由调用方决定插进哪个输入框）。
 * 转写目标：优先「设置 › 语音识别」指定的专用转写模型；未配置时用调用方给的
 * `fallbackTarget`（会话模式传当前会话/草稿所选模型；群聊没有"当前会话模型"这回事，
 * 只认设置里的配置）。
 *
 * 音频处理：浏览器录出的是 webm/opus，上传前尽量转成 16kHz 单声道 WAV ——
 * 各家 ASR 兼容层对 webm 的支持参差，WAV/PCM 通用度最高；转不了则回退原格式。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Loader2, Mic, Square } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { useVoiceInputStore } from '../../stores/voiceInputStore';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';

/** 录音时长上限（秒）：兜住"忘了点结束"的情况，也避开转写接口的体积上限 */
const MAX_RECORD_SECONDS = 120;

interface VoiceTarget {
  providerId: string;
  model: string;
  /** 是否来自设置里的专用转写模型（用于按钮提示措辞） */
  fromSettings: boolean;
}

interface VoiceInputButtonProps {
  /** 设置里未配置专用转写模型时的回退目标；传 null 表示只能靠设置里的配置 */
  fallbackTarget?: { providerId: string; model: string } | null;
  /** 转写文本回传（调用方自己决定追加到哪个输入框、并聚焦） */
  onTranscribed: (text: string) => void;
  /** 窄容器下只留图标 */
  compact?: boolean;
  /** 不可用（如群聊未选房间） */
  disabled?: boolean;
  /** 不可用时的提示文案 */
  disabledHint?: string;
}

/** Blob → base64（不含 data URL 前缀） */
function blobToBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const s = String(reader.result ?? '');
      const comma = s.indexOf(',');
      resolve(comma >= 0 ? s.slice(comma + 1) : '');
    };
    reader.onerror = () => reject(new Error('读取录音数据失败'));
    reader.readAsDataURL(blob);
  });
}

/** Float32 PCM → 16bit PCM WAV */
function encodeWav(samples: Float32Array, sampleRate: number): Blob {
  const buffer = new ArrayBuffer(44 + samples.length * 2);
  const view = new DataView(buffer);
  const writeAscii = (offset: number, s: string) => {
    for (let i = 0; i < s.length; i++) view.setUint8(offset + i, s.charCodeAt(i));
  };
  writeAscii(0, 'RIFF');
  view.setUint32(4, 36 + samples.length * 2, true);
  writeAscii(8, 'WAVE');
  writeAscii(12, 'fmt ');
  view.setUint32(16, 16, true); // fmt chunk 长度
  view.setUint16(20, 1, true); // PCM
  view.setUint16(22, 1, true); // 单声道
  view.setUint32(24, sampleRate, true);
  view.setUint32(28, sampleRate * 2, true); // 字节率
  view.setUint16(32, 2, true); // block align
  view.setUint16(34, 16, true); // 位深
  writeAscii(36, 'data');
  view.setUint32(40, samples.length * 2, true);
  let offset = 44;
  for (let i = 0; i < samples.length; i++, offset += 2) {
    const s = Math.max(-1, Math.min(1, samples[i]));
    view.setInt16(offset, s < 0 ? s * 0x8000 : s * 0x7fff, true);
  }
  return new Blob([buffer], { type: 'audio/wav' });
}

/** 录音 Blob → 16kHz 单声道 WAV（失败返回 null，调用方回退原格式） */
async function blobToWav16k(blob: Blob): Promise<Blob | null> {
  try {
    const decoded = await new AudioContext().decodeAudioData(await blob.arrayBuffer());
    const targetRate = 16000;
    const frames = Math.max(1, Math.ceil(decoded.duration * targetRate));
    const offline = new OfflineAudioContext(1, frames, targetRate);
    const source = offline.createBufferSource();
    source.buffer = decoded;
    source.connect(offline.destination);
    source.start();
    const rendered = await offline.startRendering();
    return encodeWav(rendered.getChannelData(0), targetRate);
  } catch (e) {
    console.warn('[VoiceInput] 录音转 WAV 失败，回退原始格式:', e);
    return null;
  }
}

export function VoiceInputButton({
  fallbackTarget = null,
  onTranscribed,
  compact = false,
  disabled = false,
  disabledHint,
}: VoiceInputButtonProps) {
  const voiceOverride = useVoiceInputStore((s) => s.config);

  // 配置是可选覆盖项，挂载时惰性读一次（store 内幂等）
  useEffect(() => {
    void useVoiceInputStore.getState().load();
  }, []);

  const [recording, setRecording] = useState(false);
  const [recordSeconds, setRecordSeconds] = useState(0);
  const [transcribing, setTranscribing] = useState(false);
  const recorderRef = useRef<MediaRecorder | null>(null);
  const chunksRef = useRef<Blob[]>([]);
  const streamRef = useRef<MediaStream | null>(null);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);

  // 用 useMemo 固定 target 引用：否则每次渲染都会新建对象，令 finishRecording/startRecording
  // 的依赖恒变（react-hooks/exhaustive-deps 警告）。取值逻辑与原先完全一致。
  const target: VoiceTarget | null = useMemo(
    () =>
      voiceOverride.providerId && voiceOverride.model
        ? { providerId: voiceOverride.providerId, model: voiceOverride.model, fromSettings: true }
        : fallbackTarget && fallbackTarget.providerId && fallbackTarget.model
          ? { providerId: fallbackTarget.providerId, model: fallbackTarget.model, fromSettings: false }
          : null,
    [voiceOverride.providerId, voiceOverride.model, fallbackTarget],
  );

  const clearTimer = useCallback(() => {
    if (timerRef.current) {
      clearInterval(timerRef.current);
      timerRef.current = null;
    }
  }, []);

  /** 释放麦克风与计时器：停止、异常、卸载都要走一遍，否则系统麦克风指示灯不灭 */
  const release = useCallback(() => {
    clearTimer();
    streamRef.current?.getTracks().forEach((t) => t.stop());
    streamRef.current = null;
    recorderRef.current = null;
  }, [clearTimer]);

  /** 录音结束 → 自动转写 → 文本交回调用方（不自动发送，留纠错机会） */
  const finishRecording = useCallback(async () => {
    const blob = new Blob(chunksRef.current, { type: 'audio/webm' });
    chunksRef.current = [];
    release();
    setRecording(false);
    if (!target) return;
    if (blob.size === 0) {
      showToast('没有录到音频', 'error');
      return;
    }
    setTranscribing(true);
    try {
      const wav = await blobToWav16k(blob);
      const audioBase64 = await blobToBase64(wav ?? blob);
      const text = await invoke<string>('transcribe_audio', {
        providerId: target.providerId,
        model: target.model,
        audioBase64,
        mime: wav ? 'audio/wav' : 'audio/webm',
      });
      const trimmed = text.trim();
      if (!trimmed) {
        showToast('没有识别到语音内容', 'info');
        return;
      }
      onTranscribed(trimmed);
    } catch (e) {
      showToast(`语音识别失败: ${errorMessage(e)}`, 'error');
    } finally {
      setTranscribing(false);
    }
  }, [target, release, onTranscribed]);

  /** 开始录音 */
  const startRecording = useCallback(async () => {
    if (recording || transcribing || disabled) return;
    if (!target) {
      showToast('语音输入：请先在 设置 › 语音识别 指定专用转写模型', 'info');
      return;
    }
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      streamRef.current = stream;
      const mime = MediaRecorder.isTypeSupported('audio/webm;codecs=opus') ? 'audio/webm;codecs=opus' : '';
      const recorder = new MediaRecorder(stream, mime ? { mimeType: mime } : undefined);
      chunksRef.current = [];
      recorder.ondataavailable = (e) => {
        if (e.data.size > 0) chunksRef.current.push(e.data);
      };
      recorder.onstop = () => { void finishRecording(); };
      recorderRef.current = recorder;
      recorder.start();
      setRecordSeconds(0);
      setRecording(true);
      timerRef.current = setInterval(() => setRecordSeconds((s) => s + 1), 1000);
    } catch (e) {
      release();
      showToast(
        `无法访问麦克风: ${errorMessage(e)}（请检查系统「隐私和安全性 › 麦克风」是否允许桌面应用访问）`,
        'error',
      );
    }
  }, [recording, transcribing, disabled, target, finishRecording, release]);

  /** 结束录音并自动识别 */
  const stopRecording = useCallback(() => {
    clearTimer();
    const recorder = recorderRef.current;
    setRecording(false);
    if (recorder && recorder.state !== 'inactive') recorder.stop();
  }, [clearTimer]);

  // 超过时长上限自动收尾（用户忘了点结束）。
  // effect 体内不允许同步 setState（`react-hooks/set-state-in-effect`）：stopRecording 会同步
  // setRecording，故把收尾推迟一个微任务，仍在同一帧内执行，观感与原先一致。
  useEffect(() => {
    if (recording && recordSeconds >= MAX_RECORD_SECONDS) {
      queueMicrotask(() => stopRecording());
    }
  }, [recording, recordSeconds, stopRecording]);

  // 卸载时释放麦克风
  useEffect(() => () => release(), [release]);

  const title = disabled
    ? disabledHint || '语音输入不可用'
    : transcribing
      ? '正在识别…'
      : recording
        ? `录音中 ${recordSeconds}s（上限 ${MAX_RECORD_SECONDS}s）· 点击结束并识别`
        : target
          ? `语音输入：用${target.fromSettings ? '设置里的转写模型' : '当前会话模型'} ${target.model} 转写后插入输入框`
          : '语音输入：请先在 设置 › 语音识别 指定专用转写模型';

  return (
    <button
      onClick={() => (recording ? stopRecording() : void startRecording())}
      disabled={disabled || transcribing}
      className="flex items-center gap-1 px-2 py-1 rounded-lg text-xs transition-colors shrink-0"
      style={{
        color: recording
          ? 'var(--status-danger, #ef4444)'
          : disabled || transcribing
            ? 'var(--text-tertiary)'
            : 'var(--text-secondary)',
        backgroundColor: recording ? 'rgba(239,68,68,0.12)' : 'transparent',
        cursor: disabled ? 'not-allowed' : transcribing ? 'wait' : 'pointer',
        opacity: disabled ? 0.6 : 1,
      }}
      title={title}
    >
      {transcribing ? (
        <Loader2 size={12} className="animate-spin" />
      ) : recording ? (
        <>
          <Square size={10} fill="currentColor" />
          <span className="tabular-nums">{recordSeconds}s</span>
        </>
      ) : (
        <>
          <Mic size={12} />
          {!compact && '语音'}
        </>
      )}
    </button>
  );
}
