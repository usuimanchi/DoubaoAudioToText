// 豆包语音转文字 — 前端逻辑

const { invoke } = window.__TAURI__?.core || {};
const { listen } = window.__TAURI__?.event || {};
const dialog = window.__TAURI__?.dialog || {};
const log = document.getElementById('log');
const resultList = document.getElementById('result-list');
const startBtn = document.getElementById('start-btn');
const fileList = document.getElementById('file-list');
const dropZone = document.getElementById('drop-zone');
const fileInput = document.getElementById('file-input');
const providerSelect = document.getElementById('provider');

let collectedFiles = [];
let currentJobId = null;

// ====== 初始化 ======
window.addEventListener('DOMContentLoaded', async () => {
  // 尝试加载已保存的 API Key
  try {
    const saved = await invoke('load_last_api_key');
    if (saved) {
      document.getElementById('api-key').value = saved;
      document.getElementById('remember-key').checked = true;
    }
  } catch (_) { /* 无已保存的 key */ }

  // 提供商切换（显示/隐藏 Azure 配置）
  providerSelect.onchange = toggleProviderUI;
});

// ====== 提供商 UI 切换 ======
function toggleProviderUI() {
  const isAzure = providerSelect.value === 'azure';
  document.getElementById('ark-key-group').style.display = isAzure ? 'none' : 'block';
  document.getElementById('azure-key-group').style.display = isAzure ? 'block' : 'none';
}

// ====== 文件拖拽/选择 ======
dropZone.onclick = () => fileInput.click();

dropZone.ondragover = (e) => { e.preventDefault(); dropZone.classList.add('drag-over'); };
dropZone.ondragleave = () => dropZone.classList.remove('drag-over');
dropZone.ondrop = (e) => {
  e.preventDefault();
  dropZone.classList.remove('drag-over');
  if (e.dataTransfer.files) {
    addFiles(Array.from(e.dataTransfer.files));
  }
};

fileInput.onchange = () => {
  if (fileInput.files) addFiles(Array.from(fileInput.files));
};

function addFiles(files) {
  files.forEach(f => {
    // 在 Tauri 中，文件路径从 webkitRelativePath 或直接读取
    // 对于从 <input type="file"> 选中的文件，使用其 path 属性
    const p = f.path || f.webkitRelativePath || f.name;
    if (!collectedFiles.includes(p)) {
      collectedFiles.push(p);
    }
  });
  renderFileList();
}

function renderFileList() {
  fileList.innerHTML = collectedFiles.map(p => `<li>📄 ${p}</li>`).join('');
}

// 同时监听 Tauri 拖拽事件（从文件管理器拖入）
if (window.__TAURI__) {
  window.__TAURI__.event.listen('tauri://drag-drop', (event) => {
    if (event.payload?.paths) {
      addFiles(event.payload.paths);
    }
  });
}

// ====== 选择输出目录 ======
document.getElementById('pick-dir').onclick = async () => {
  if (!dialog) { alert('目录选择仅在桌面应用中可用'); return; }
  try {
    const selected = await dialog.open({ directory: true, multiple: false });
    if (selected) {
      document.getElementById('output-dir').value = selected;
    }
  } catch (e) {
    console.error('选择目录失败:', e);
  }
};

// ====== 收集语言 ======
function collectLanguages() {
  const checked = document.querySelectorAll('.lang-chip input:checked');
  const langs = Array.from(checked)
    .map(cb => cb.value)
    .filter(v => v !== 'auto');
  return langs.join(',') || '';
}

// ====== 开始转写 ======
startBtn.onclick = async () => {
  if (!invoke) {
    appendLog('error', '❌ 请在桌面应用中运行（非浏览器）。');
    return;
  }
  if (collectedFiles.length === 0) {
    appendLog('error', '❌ 请先添加音频文件。');
    return;
  }

  const provider = providerSelect.value;
  const apiKey = document.getElementById('api-key').value.trim();
  const azureKey = document.getElementById('azure-key').value.trim();
  const azureRegion = document.getElementById('azure-region').value.trim();
  const language = collectLanguages();
  const outputDir = document.getElementById('output-dir').value.trim();
  const remember = document.getElementById('remember-key').checked;

  // 保存 API Key
  if (remember && apiKey) {
    try { await invoke('save_last_api_key', { key: apiKey }); } catch (_) {}
  }

  // 构建配置
  const config = {
    provider,
    api_key: apiKey || null,
    azure_key: azureKey || null,
    azure_region: azureRegion || null,
    language: language || null,
    output_dir: outputDir || null,
    prepare_only: false,
    ark_model: null,
  };

  // 禁用按钮
  startBtn.disabled = true;
  startBtn.textContent = '⏳ 处理中...';
  document.getElementById('log').textContent = '';
  document.getElementById('result-list').innerHTML = '';
  resultList.innerHTML = '';

  appendLog('info', `🚀 开始转写，共 ${collectedFiles.length} 个文件...`);

  try {
    currentJobId = await invoke('start_transcription', {
      configInput: config,
      inputs: collectedFiles,
    });
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
    const { event } = e.payload;
    if (event.kind === 'log') {
      appendLog(event.level, event.msg);
    } else if (event.kind === 'progress') {
      appendLog('info', `[进度] ${event.key}: ${event.pos}/${event.len}`);
    } else if (event.kind === 'result') {
      appendLog('success', `📝 ${event.path}`);
    }
  });

  listen('job_done', (e) => {
    const { success, error, output_dir } = e.payload;
    if (success) {
      appendLog('success', `\n🎉 转写完成！输出目录: ${output_dir}`);
      // 显示结果文件
      const filename = output_dir.replace(/\\/g, '/').split('/').pop() || '';
      addResultItem(output_dir, '查看结果目录');
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
  li.innerHTML = `<span>📁 ${label}</span>`;
  const openBtn = document.createElement('button');
  openBtn.textContent = '打开';
  openBtn.onclick = () => invoke('open_path', { path });
  li.appendChild(openBtn);
  resultList.appendChild(li);
}
