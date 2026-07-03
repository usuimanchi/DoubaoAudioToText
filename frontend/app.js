// 豆包语音转文字 — 前端逻辑

const { invoke } = window.__TAURI__?.core || {};
const { listen } = window.__TAURI__?.event || {};

const log = document.getElementById('log');
const resultList = document.getElementById('result-list');
const startBtn = document.getElementById('start-btn');
const fileList = document.getElementById('file-list');
const dropZone = document.getElementById('drop-zone');
const providerSelect = document.getElementById('provider');
const outputDirInput = document.getElementById('output-dir');

let collectedFiles = [];
let currentJobId = null;

// ====== 初始化 ======
window.addEventListener('DOMContentLoaded', async () => {
  if (!invoke) return;

  // 默认输出目录
  try {
    const def = await invoke('get_default_output_dir');
    outputDirInput.placeholder = def;
  } catch (_) { outputDirInput.placeholder = './结果'; }

  // 加载已保存的 API Key
  try {
    const saved = await invoke('load_last_api_key');
    if (saved) {
      document.getElementById('api-key').value = saved;
      document.getElementById('remember-key').checked = true;
    }
  } catch (_) { /* 无已保存的 key */ }

  providerSelect.onchange = toggleProviderUI;
});

// ====== 提供商 UI 切换 ======
function toggleProviderUI() {
  const isAzure = providerSelect.value === 'azure';
  document.getElementById('ark-key-group').style.display = isAzure ? 'none' : 'block';
  document.getElementById('azure-key-group').style.display = isAzure ? 'block' : 'none';
}

// ====== 文件拖拽/选择 ======
dropZone.onclick = async () => {
  try {
    const paths = await invoke('pick_audio_files');
    if (paths && paths.length > 0) addFiles(paths);
  } catch (e) { console.error('选择文件失败:', e); }
};

dropZone.ondragover = (e) => { e.preventDefault(); dropZone.classList.add('drag-over'); };
dropZone.ondragleave = () => dropZone.classList.remove('drag-over');

// Tauri 原生拖拽（从文件管理器拖入，携带完整路径）
if (window.__TAURI__) {
  window.__TAURI__.event.listen('tauri://drag-drop', (event) => {
    if (event.payload?.paths) addFiles(event.payload.paths);
  });
}

function addFiles(paths) {
  paths.forEach(p => { if (!collectedFiles.includes(p)) collectedFiles.push(p); });
  renderFileList();
}

function renderFileList() {
  fileList.innerHTML = collectedFiles.map(p => `<li>📄 ${p.replace(/\\/g, '/').split('/').pop()}</li>`).join('');
}

// ====== 选择输出目录 ======
document.getElementById('pick-dir').onclick = async () => {
  try {
    const selected = await invoke('pick_directory');
    if (selected) outputDirInput.value = selected;
  } catch (e) { console.error('选择目录失败:', e); }
};

// ====== 收集语言 ======
function collectLanguages() {
  const checked = document.querySelectorAll('.lang-chip input:checked');
  const langs = Array.from(checked).map(cb => cb.value).filter(v => v !== 'auto');
  return langs.join(',') || '';
}

// ====== 开始转写 ======
startBtn.onclick = async () => {
  if (!invoke) { appendLog('error', '❌ 请在桌面应用中运行（非浏览器）。'); return; }
  if (collectedFiles.length === 0) { appendLog('error', '❌ 请先添加音频文件。'); return; }

  const provider = providerSelect.value;
  const apiKey = document.getElementById('api-key').value.trim();
  const azureKey = document.getElementById('azure-key').value.trim();
  const azureRegion = document.getElementById('azure-region').value.trim();

  // 校验
  if (provider !== 'azure' && !apiKey) { appendLog('error', '❌ 请输入 API Key。'); return; }
  if (provider === 'azure' && (!azureKey || !azureRegion)) { appendLog('error', '❌ 请输入 Azure Key 和区域。'); return; }

  const language = collectLanguages();
  const outputDir = outputDirInput.value.trim() || outputDirInput.placeholder;
  const remember = document.getElementById('remember-key').checked;

  // 保存 API Key
  if (remember && apiKey) {
    try { await invoke('save_last_api_key', { key: apiKey }); } catch (_) {}
  }

  const config = { provider, api_key: apiKey || null, azure_key: azureKey || null,
    azure_region: azureRegion || null, language: language || null,
    output_dir: outputDir || null, prepare_only: false, ark_model: null };

  startBtn.disabled = true;
  startBtn.textContent = '⏳ 处理中...';
  document.getElementById('log').textContent = '';
  document.getElementById('result-list').innerHTML = '';

  appendLog('info', `🚀 开始转写，共 ${collectedFiles.length} 个文件...`);

  try {
    currentJobId = await invoke('start_transcription', { configInput: config, inputs: collectedFiles });
    appendLog('info', `任务 ID: ${currentJobId}`);
  } catch (e) {
    appendLog('error', `❌ 提交失败: ${e}`);
    startBtn.disabled = false;
    startBtn.textContent = '🚀 开始转写';
  }
};

// ====== 监听进度 ======
if (listen) {
  listen('progress', (e) => {
    const evt = e.payload?.event;
    if (!evt) return;
    if (evt.kind === 'log') {
      appendLog(evt.level, evt.msg);
    } else if (evt.kind === 'progress') {
      appendLog('info', `[进度] ${evt.pos}/${evt.len}`);
    }
  });

  listen('job_done', (e) => {
    const { success, error, output_dir } = e.payload;
    if (success) {
      appendLog('success', `\n🎉 转写完成！输出目录: ${output_dir}`);
      addResultItem(output_dir, '打开结果目录');
    } else {
      appendLog('error', `\n❌ 任务失败: ${error}`);
    }
    startBtn.disabled = false;
    startBtn.textContent = '🚀 开始转写';
    currentJobId = null;
  });
}

// ====== 辅助函数 ======
function appendLog(level, msg) {
  const pre = document.getElementById('log');
  if (!pre) return;
  const span = document.createElement('span');
  span.className = `log-${level}`;
  span.textContent = msg + '\n';
  pre.appendChild(span);
  pre.scrollTop = pre.scrollHeight;
}

function addResultItem(path, label) {
  const li = document.createElement('li');
  li.innerHTML = `<span>📁 ${label}: ${path}</span>`;
  const openBtn = document.createElement('button');
  openBtn.textContent = '打开';
  openBtn.onclick = () => invoke('open_path', { path });
  li.appendChild(openBtn);
  resultList.appendChild(li);
}
