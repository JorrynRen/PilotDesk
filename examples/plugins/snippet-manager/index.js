/**
 * Snippet Manager — 文本片段管理插件
 *
 * 功能：
 * 1. 保存常用文本片段和提示词模板
 * 2. 按分类管理（prompt / code / template / general）
 * 3. 搜索过滤
 * 4. 一键复制到剪贴板
 * 5. 数据持久化（通过 PluginAPI.storage）
 */

// ── 样式常量 ──
var STYLE = {
  container: {
    padding: '12px',
    height: '100%',
    display: 'flex',
    flexDirection: 'column',
    gap: '8px',
    fontSize: '12px',
  },
  searchInput: {
    width: '100%',
    padding: '6px 10px',
    borderRadius: '6px',
    border: '1px solid var(--border)',
    backgroundColor: 'var(--bg-primary)',
    color: 'var(--text-primary)',
    fontSize: '12px',
    outline: 'none',
    boxSizing: 'border-box',
  },
  addBtn: {
    padding: '6px 12px',
    borderRadius: '6px',
    border: 'none',
    backgroundColor: 'var(--accent)',
    color: '#fff',
    fontSize: '11px',
    cursor: 'pointer',
    fontWeight: 500,
    whiteSpace: 'nowrap',
  },
  snippetCard: {
    padding: '8px 10px',
    borderRadius: '6px',
    backgroundColor: 'var(--bg-primary)',
    border: '1px solid var(--border)',
    cursor: 'pointer',
    transition: 'border-color 0.15s',
  },
  snippetTitle: {
    fontSize: '12px',
    fontWeight: 600,
    color: 'var(--text-primary)',
    marginBottom: '2px',
  },
  snippetPreview: {
    fontSize: '11px',
    color: 'var(--text-tertiary)',
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  categoryTag: {
    display: 'inline-block',
    padding: '1px 6px',
    borderRadius: '4px',
    fontSize: '9px',
    fontWeight: 500,
    marginRight: '4px',
  },
  copyBtn: {
    padding: '2px 8px',
    borderRadius: '4px',
    border: '1px solid var(--border)',
    backgroundColor: 'transparent',
    color: 'var(--text-secondary)',
    fontSize: '10px',
    cursor: 'pointer',
  },
  emptyState: {
    textAlign: 'center',
    padding: '24px 12px',
    color: 'var(--text-tertiary)',
    fontSize: '12px',
  },
  modalOverlay: {
    position: 'fixed',
    inset: 0,
    backgroundColor: 'rgba(0,0,0,0.4)',
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    zIndex: 9999,
  },
  modalContent: {
    backgroundColor: 'var(--bg-primary)',
    borderRadius: '10px',
    padding: '16px',
    width: '320px',
    maxWidth: '90vw',
    boxShadow: '0 8px 32px rgba(0,0,0,0.2)',
  },
  modalTitle: {
    fontSize: '14px',
    fontWeight: 600,
    color: 'var(--text-primary)',
    marginBottom: '12px',
  },
  fieldLabel: {
    fontSize: '11px',
    color: 'var(--text-secondary)',
    marginBottom: '4px',
    display: 'block',
  },
  fieldInput: {
    width: '100%',
    padding: '6px 8px',
    borderRadius: '6px',
    border: '1px solid var(--border)',
    backgroundColor: 'var(--bg-secondary)',
    color: 'var(--text-primary)',
    fontSize: '12px',
    outline: 'none',
    boxSizing: 'border-box',
    marginBottom: '8px',
  },
  fieldTextarea: {
    width: '100%',
    padding: '6px 8px',
    borderRadius: '6px',
    border: '1px solid var(--border)',
    backgroundColor: 'var(--bg-secondary)',
    color: 'var(--text-primary)',
    fontSize: '12px',
    outline: 'none',
    resize: 'vertical',
    minHeight: '80px',
    boxSizing: 'border-box',
    marginBottom: '8px',
    fontFamily: 'inherit',
  },
  selectField: {
    width: '100%',
    padding: '6px 8px',
    borderRadius: '6px',
    border: '1px solid var(--border)',
    backgroundColor: 'var(--bg-secondary)',
    color: 'var(--text-primary)',
    fontSize: '12px',
    outline: 'none',
    boxSizing: 'border-box',
    marginBottom: '8px',
  },
  modalActions: {
    display: 'flex',
    justifyContent: 'flex-end',
    gap: '8px',
    marginTop: '4px',
  },
  cancelBtn: {
    padding: '6px 14px',
    borderRadius: '6px',
    border: '1px solid var(--border)',
    backgroundColor: 'transparent',
    color: 'var(--text-secondary)',
    fontSize: '11px',
    cursor: 'pointer',
  },
  saveBtn: {
    padding: '6px 14px',
    borderRadius: '6px',
    border: 'none',
    backgroundColor: 'var(--accent)',
    color: '#fff',
    fontSize: '11px',
    cursor: 'pointer',
    fontWeight: 500,
  },
  deleteBtn: {
    padding: '2px 6px',
    borderRadius: '4px',
    border: 'none',
    backgroundColor: 'transparent',
    color: 'var(--text-tertiary)',
    fontSize: '10px',
    cursor: 'pointer',
  },
  scrollArea: {
    flex: 1,
    overflowY: 'auto',
    display: 'flex',
    flexDirection: 'column',
    gap: '6px',
  },
  headerRow: {
    display: 'flex',
    alignItems: 'center',
    gap: '8px',
  },
  catPrompt: { backgroundColor: '#e8f5e9', color: '#2e7d32' },
  catCode: { backgroundColor: '#e3f2fd', color: '#1565c0' },
  catTemplate: { backgroundColor: '#fff3e0', color: '#e65100' },
  catGeneral: { backgroundColor: '#f3e5f5', color: '#7b1fa2' },
};

var CATEGORIES = [
  { id: 'prompt', label: 'Prompt', style: STYLE.catPrompt },
  { id: 'code', label: 'Code', style: STYLE.catCode },
  { id: 'template', label: 'Template', style: STYLE.catTemplate },
  { id: 'general', label: 'General', style: STYLE.catGeneral },
];

var CATEGORY_MAP = {};
CATEGORIES.forEach(function(c) { CATEGORY_MAP[c.id] = c; });

// ── 工具函数 ──
function getCategoryStyle(catId) {
  var cat = CATEGORY_MAP[catId];
  return cat ? cat.style : STYLE.catGeneral;
}

function getCategoryLabel(catId) {
  var cat = CATEGORY_MAP[catId];
  return cat ? cat.label : 'General';
}

// ── 模态框组件 ──
function SnippetFormModal(props) {
  var snippet = props.snippet;
  var onSave = props.onSave;
  var onCancel = props.onCancel;

  var titleState = React.useState(snippet ? snippet.title : '');
  var contentState = React.useState(snippet ? snippet.content : '');
  var categoryState = React.useState(snippet ? snippet.category : 'prompt');
  var title = titleState[0];
  var setTitle = titleState[1];
  var content = contentState[0];
  var setContent = contentState[1];
  var category = categoryState[0];
  var setCategory = categoryState[1];

  function handleSave() {
    if (!title.trim() || !content.trim()) return;
    onSave({
      title: title.trim(),
      content: content.trim(),
      category: category,
    });
  }

  return React.createElement('div', {
    style: STYLE.modalOverlay,
    onClick: function(e) { if (e.target === e.currentTarget) onCancel(); },
  },
    React.createElement('div', { style: STYLE.modalContent, onClick: function(e) { e.stopPropagation(); } },
      React.createElement('div', { style: STYLE.modalTitle },
        snippet ? 'Edit Snippet' : 'New Snippet'
      ),

      React.createElement('label', { style: STYLE.fieldLabel }, 'Title'),
      React.createElement('input', {
        style: STYLE.fieldInput,
        value: title,
        onChange: function(e) { setTitle(e.target.value); },
        placeholder: 'e.g. Code Review Prompt',
        autoFocus: true,
      }),

      React.createElement('label', { style: STYLE.fieldLabel }, 'Category'),
      React.createElement('select', {
        style: STYLE.selectField,
        value: category,
        onChange: function(e) { setCategory(e.target.value); },
      },
        CATEGORIES.map(function(c) {
          return React.createElement('option', { key: c.id, value: c.id }, c.label);
        })
      ),

      React.createElement('label', { style: STYLE.fieldLabel }, 'Content'),
      React.createElement('textarea', {
        style: STYLE.fieldTextarea,
        value: content,
        onChange: function(e) { setContent(e.target.value); },
        placeholder: 'Paste your snippet content here...',
      }),

      React.createElement('div', { style: STYLE.modalActions },
        React.createElement('button', { style: STYLE.cancelBtn, onClick: onCancel }, 'Cancel'),
        React.createElement('button', {
          style: STYLE.saveBtn,
          onClick: handleSave,
        }, snippet ? 'Update' : 'Save')
      )
    )
  );
}

// ── 主面板组件 ──
function SnippetPanel(props) {
  var api = props.api;

  // 状态
  var snippetsState = React.useState([]);
  var searchState = React.useState('');
  var showFormState = React.useState(false);
  var editingState = React.useState(null);

  var snippets = snippetsState[0];
  var setSnippets = snippetsState[1];
  var search = searchState[0];
  var setSearch = searchState[1];
  var showForm = showFormState[0];
  var setShowForm = showFormState[1];
  var editing = editingState[0];
  var setEditing = editingState[1];

  // 加载数据
  React.useEffect(function() {
    api.storage.get('snippets').then(function(data) {
      if (data) {
        try {
          setSnippets(JSON.parse(data));
        } catch(e) {
          setSnippets([]);
        }
      }
    });
  }, []);

  // 保存数据
  function persistSnippets(list) {
    setSnippets(list);
    api.storage.set('snippets', JSON.stringify(list));
  }

  // 添加/更新
  function handleSave(data) {
    var list = snippets.slice();
    if (editing) {
      // 更新
      for (var i = 0; i < list.length; i++) {
        if (list[i].id === editing.id) {
          list[i] = { id: editing.id, title: data.title, content: data.content, category: data.category, createdAt: editing.createdAt };
          break;
        }
      }
      api.ui.showToast('Snippet updated', 'success');
    } else {
      // 新增
      list.unshift({
        id: Date.now().toString(36) + Math.random().toString(36).slice(2, 6),
        title: data.title,
        content: data.content,
        category: data.category,
        createdAt: Date.now(),
      });
      api.ui.showToast('Snippet saved', 'success');
    }
    persistSnippets(list);
    setShowForm(false);
    setEditing(null);
  }

  // 删除
  function handleDelete(id) {
    var list = snippets.filter(function(s) { return s.id !== id; });
    persistSnippets(list);
    api.ui.showToast('Snippet deleted', 'info');
  }

  // 复制到剪贴板
  function handleCopy(content) {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(content).then(function() {
        api.ui.showToast('Copied to clipboard', 'success');
      });
    }
  }

  // 过滤
  var filtered = snippets;
  if (search.trim()) {
    var q = search.trim().toLowerCase();
    filtered = snippets.filter(function(s) {
      return s.title.toLowerCase().indexOf(q) !== -1
        || s.content.toLowerCase().indexOf(q) !== -1
        || s.category.indexOf(q) !== -1;
    });
  }

  // 分类统计
  var counts = { prompt: 0, code: 0, template: 0, general: 0 };
  snippets.forEach(function(s) {
    if (counts[s.category] !== undefined) counts[s.category]++;
    else counts.general++;
  });

  return React.createElement('div', { style: STYLE.container },

    // 头部：搜索 + 添加按钮
    React.createElement('div', { style: STYLE.headerRow },
      React.createElement('input', {
        style: STYLE.searchInput,
        value: search,
        onChange: function(e) { setSearch(e.target.value); },
        placeholder: 'Search snippets...',
      }),
      React.createElement('button', {
        style: STYLE.addBtn,
        onClick: function() { setEditing(null); setShowForm(true); },
      }, '+ New')
    ),

    // 分类统计
    React.createElement('div', { style: { display: 'flex', gap: '4px', flexWrap: 'wrap' } },
      CATEGORIES.map(function(c) {
        var count = counts[c.id] || 0;
        if (count === 0) return null;
        return React.createElement('span', {
          key: c.id,
          style: Object.assign({}, c.style, { fontSize: '9px', padding: '1px 6px', borderRadius: '4px' }),
        }, c.label + ' ' + count);
      })
    ),

    // 片段列表
    React.createElement('div', { style: STYLE.scrollArea },
      filtered.length === 0
        ? React.createElement('div', { style: STYLE.emptyState },
            search.trim()
              ? 'No snippets match "' + search + '"'
              : 'No snippets yet.\nClick "+ New" to add your first snippet.'
          )
        : filtered.map(function(s) {
            return React.createElement('div', {
              key: s.id,
              style: STYLE.snippetCard,
              onMouseEnter: function(e) {
                e.currentTarget.style.borderColor = 'var(--accent)';
              },
              onMouseLeave: function(e) {
                e.currentTarget.style.borderColor = 'var(--border)';
              },
            },
              // 标题行
              React.createElement('div', { style: { display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '2px' } },
                React.createElement('div', { style: { display: 'flex', alignItems: 'center', gap: '4px', minWidth: 0 } },
                  React.createElement('span', { style: getCategoryStyle(s.category) }, getCategoryLabel(s.category)),
                  React.createElement('span', { style: STYLE.snippetTitle }, s.title),
                ),
                React.createElement('div', { style: { display: 'flex', gap: '4px', flexShrink: 0 } },
                  React.createElement('button', {
                    style: STYLE.copyBtn,
                    onClick: function(e) { e.stopPropagation(); handleCopy(s.content); },
                    title: 'Copy to clipboard',
                  }, 'Copy'),
                  React.createElement('button', {
                    style: STYLE.deleteBtn,
                    onClick: function(e) { e.stopPropagation(); handleDelete(s.id); },
                    title: 'Delete',
                  }, '✕'),
                )
              ),
              // 内容预览
              React.createElement('div', { style: STYLE.snippetPreview },
                s.content.length > 120 ? s.content.slice(0, 120) + '...' : s.content
              ),
              // 编辑按钮
              React.createElement('div', {
                style: { textAlign: 'right', marginTop: '2px' },
                onClick: function(e) { e.stopPropagation(); },
              },
                React.createElement('button', {
                  style: { ...STYLE.deleteBtn, color: 'var(--text-tertiary)', fontSize: '9px', textDecoration: 'underline' },
                  onClick: function() { setEditing(s); setShowForm(true); },
                }, 'Edit')
              )
            );
          })
    ),

    // 模态框
    showForm
      ? React.createElement(SnippetFormModal, {
          snippet: editing,
          onSave: handleSave,
          onCancel: function() { setShowForm(false); setEditing(null); },
        })
      : null
  );
}

// ── 插件入口 ──
export default {
  onLoad: function(api) {
    console.log('[SnippetManager] Plugin loaded');

    api.ui.addPanel({
      id: 'snippets',
      title: 'Snippets',
      component: function() {
        return React.createElement(SnippetPanel, { api: api });
      },
    });

    api.ui.showToast('Snippet Manager loaded', 'success');
  },

  onUnload: function() {
    console.log('[SnippetManager] Plugin unloaded');
  },
};
