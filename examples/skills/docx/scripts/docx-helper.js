'use strict';

const {
    Document, Packer, Paragraph, TextRun, Table, TableRow, TableCell,
    Header, Footer, HeadingLevel, BorderStyle, WidthType, ShadingType,
    AlignmentType, PageNumber, PageBreak, ImageRun, LevelFormat,
    ExternalHyperlink, InternalHyperlink, Bookmark, FootnoteReferenceRun,
    PageOrientation, TableOfContents, VerticalMergeType, VerticalAlign, TextDirection,
    PositionalTab, PositionalTabAlignment, PositionalTabRelativeTo, PositionalTabLeader,
    TabStopType, TabStopPosition, Column, SectionType, LineRuleType, HeightRule,
    HorizontalPositionRelativeFrom, VerticalPositionRelativeFrom,
    TableAnchorType, OverlapType, RelativeHorizontalPosition,
} = require('docx');
const fs = require('fs');
const path = require('path');

// ── Defaults ────────────────────────────────────────────────────

const DEFAULTS = {
    fonts: { heading: 'Microsoft YaHei', body: 'Microsoft YaHei' },
    sizes: { h1: 36, h2: 30, h3: 26, body: 22, small: 18 },
    colors: {
        primary: '2B579A', text: '333333', light: 'F2F6FC',
        white: 'FFFFFF', border: 'D0D0D0',
    },
    spacing: {
        heading: { before: 360, after: 200 },
        body: { after: 160, line: 380 },
    },
    page: {
        width: 12240, height: 15840,
        margins: { top: 1440, right: 1440, bottom: 1440, left: 1440 },
    },
};

const PRESETS = {};

function deepMerge(target, source) {
    const result = { ...target };
    for (const key of Object.keys(source)) {
        if (
            source[key] && typeof source[key] === 'object' && !Array.isArray(source[key]) &&
            target[key] && typeof target[key] === 'object' && !Array.isArray(target[key])
        ) {
            result[key] = deepMerge(target[key], source[key]);
        } else if (source[key] !== undefined) {
            result[key] = source[key];
        }
    }
    return result;
}

function _int(v) {
    if (v == null) return v;
    const n = Number(v);
    return Number.isFinite(n) ? Math.round(n) : v;
}

function _intArr(arr) {
    return Array.isArray(arr) ? arr.map(v => _int(v)) : arr;
}

function _intObj(obj) {
    if (!obj || typeof obj !== 'object') return obj;
    const out = {};
    for (const k of Object.keys(obj)) {
        out[k] = typeof obj[k] === 'object' && obj[k] !== null ? _intObj(obj[k]) : _int(obj[k]);
    }
    return out;
}

const ALIGN = {
    left: AlignmentType.LEFT,
    center: AlignmentType.CENTER,
    right: AlignmentType.RIGHT,
    justify: AlignmentType.JUSTIFIED,
};

const V_ALIGN = {
    top: VerticalAlign.TOP,
    center: VerticalAlign.CENTER,
    middle: VerticalAlign.CENTER,
    bottom: VerticalAlign.BOTTOM,
};

const TEXT_DIRECTION_RAW = TextDirection || {
    LEFT_TO_RIGHT_TOP_TO_BOTTOM: 'lrTb',
    TOP_TO_BOTTOM_RIGHT_TO_LEFT: 'tbRl',
    BOTTOM_TO_TOP_LEFT_TO_RIGHT: 'btLr',
};

const TEXT_DIR = {
    lrTb: TEXT_DIRECTION_RAW.LEFT_TO_RIGHT_TOP_TO_BOTTOM,
    lrtb: TEXT_DIRECTION_RAW.LEFT_TO_RIGHT_TOP_TO_BOTTOM,
    normal: TEXT_DIRECTION_RAW.LEFT_TO_RIGHT_TOP_TO_BOTTOM,
    tbRl: TEXT_DIRECTION_RAW.TOP_TO_BOTTOM_RIGHT_TO_LEFT,
    tbrl: TEXT_DIRECTION_RAW.TOP_TO_BOTTOM_RIGHT_TO_LEFT,
    verticalRl: TEXT_DIRECTION_RAW.TOP_TO_BOTTOM_RIGHT_TO_LEFT,
    'vertical-rl': TEXT_DIRECTION_RAW.TOP_TO_BOTTOM_RIGHT_TO_LEFT,
    clockwise: TEXT_DIRECTION_RAW.TOP_TO_BOTTOM_RIGHT_TO_LEFT,
    btLr: TEXT_DIRECTION_RAW.BOTTOM_TO_TOP_LEFT_TO_RIGHT,
    btlr: TEXT_DIRECTION_RAW.BOTTOM_TO_TOP_LEFT_TO_RIGHT,
    verticalLr: TEXT_DIRECTION_RAW.BOTTOM_TO_TOP_LEFT_TO_RIGHT,
    'vertical-lr': TEXT_DIRECTION_RAW.BOTTOM_TO_TOP_LEFT_TO_RIGHT,
    counterclockwise: TEXT_DIRECTION_RAW.BOTTOM_TO_TOP_LEFT_TO_RIGHT,
};

function _mapVerticalAlign(value) {
    if (!value || typeof value !== 'string') return value;
    return V_ALIGN[value] || V_ALIGN[value.toLowerCase()] || value;
}

function _mapTextDirection(value) {
    if (!value || typeof value !== 'string') return value;
    return TEXT_DIR[value] || TEXT_DIR[value.toLowerCase()] || value;
}

function _firstDefined(obj, keys) {
    if (!obj || typeof obj !== 'object') return undefined;
    for (const k of keys) {
        if (obj[k] !== undefined) return obj[k];
    }
    return undefined;
}

const HEIGHT_RULE_RAW = HeightRule || {
    AUTO: 'auto',
    ATLEAST: 'atLeast',
    EXACT: 'exact',
};

const ROW_HEIGHT_RULE = {
    auto: HEIGHT_RULE_RAW.AUTO,
    atLeast: HEIGHT_RULE_RAW.ATLEAST,
    atleast: HEIGHT_RULE_RAW.ATLEAST,
    exact: HEIGHT_RULE_RAW.EXACT,
};

function _mapRowHeightRule(value) {
    if (!value || typeof value !== 'string') return value;
    return ROW_HEIGHT_RULE[value] || ROW_HEIGHT_RULE[value.toLowerCase()] || value;
}

// ── Factory ─────────────────────────────────────────────────────

module.exports = function createHelpers(userConfig) {
    const uc = userConfig || {};
    const presetName = uc.preset;
    const base = presetName && PRESETS[presetName] ? deepMerge(DEFAULTS, PRESETS[presetName]) : DEFAULTS;
    const cfg = deepMerge(base, uc);
    cfg.sizes = _intObj(cfg.sizes);
    cfg.spacing = _intObj(cfg.spacing);
    cfg.page = _intObj(cfg.page);
    if (cfg.fonts.english) {
        const en = cfg.fonts.english;
        if (typeof cfg.fonts.body === 'string') {
            cfg.fonts.body = { ascii: en, eastAsia: cfg.fonts.body, hAnsi: en, cs: en };
        }
        if (typeof cfg.fonts.heading === 'string') {
            cfg.fonts.heading = { ascii: en, eastAsia: cfg.fonts.heading, hAnsi: en, cs: en };
        }
    }

    if (cfg.indent && typeof cfg.indent === 'object') {
        cfg.indent = _int(cfg.indent.firstLine) || _int(cfg.indent.left) || 0;
    }

    const _fullContentWidth = _int(cfg.page.width - cfg.page.margins.left - cfg.page.margins.right);
    const _columnCount = (cfg.columns && cfg.columns.count) || 1;
    const _columnSpace = (cfg.columns && cfg.columns.space) || 708;
    const _columnContentWidth = _columnCount > 1
        ? Math.floor((_fullContentWidth - _columnSpace * (_columnCount - 1)) / _columnCount)
        : _fullContentWidth;
    let contentWidth = _columnContentWidth;
    const _EMOJI_RE = /\p{Extended_Pictographic}(?:\uFE0E|\uFE0F)?(?:\u200D\p{Extended_Pictographic}(?:\uFE0E|\uFE0F)?)*/gu;
    const _EMOJI_FONT = cfg.fonts.emoji || 'Segoe UI Emoji';

    function _spreadableBlock(block) {
        if (
            block
            && (block instanceof Paragraph || block instanceof Table)
            && typeof block[Symbol.iterator] !== 'function'
        ) {
            Object.defineProperty(block, Symbol.iterator, {
                value: function* () { yield this; },
                enumerable: false,
                configurable: true,
            });
        }
        return block;
    }

    // ── Numbering presets ─────────────────────────────────────────

    const BULLET_REF = '__hlp_bullet';
    const NUMBER_REF = '__hlp_number';
    const _usedNumberingRefs = new Set();
    const _builtinNumbering = [
        {
            reference: BULLET_REF,
            levels: [
                {
                    level: 0, format: LevelFormat.BULLET, text: '\u2022', alignment: AlignmentType.LEFT,
                    style: { paragraph: { indent: { left: 720, hanging: 360 } } }
                },
                {
                    level: 1, format: LevelFormat.BULLET, text: '\u25E6', alignment: AlignmentType.LEFT,
                    style: { paragraph: { indent: { left: 1440, hanging: 360 } } }
                },
            ],
        },
        {
            reference: NUMBER_REF,
            levels: [
                {
                    level: 0, format: LevelFormat.DECIMAL, text: '%1.', alignment: AlignmentType.LEFT,
                    style: { paragraph: { indent: { left: 720, hanging: 360 } } }
                },
                {
                    level: 1, format: LevelFormat.LOWER_LETTER, text: '%2)', alignment: AlignmentType.LEFT,
                    style: { paragraph: { indent: { left: 1440, hanging: 360 } } }
                },
            ],
        },
    ];

    // ── TextRun helpers ───────────────────────────────────────────

    function _runProps(props) {
        const rp = {};
        if (props.font || !props._noDefaultFont) rp.font = props.font || cfg.fonts.body;
        if (props.size || !props._noDefaultSize) rp.size = _int(props.size) || cfg.sizes.body;
        if (props.color || !props._noDefaultColor) rp.color = props.color || cfg.colors.text;
        if (props.bold) rp.bold = true;
        if (props.italics || props.italic) rp.italics = true;
        if (props.underline) rp.underline = typeof props.underline === 'object' ? props.underline : { type: 'single' };
        if (props.strike) rp.strike = true;
        if (props.highlight) rp.highlight = props.highlight;
        if (props.superScript) rp.superScript = true;
        if (props.subScript) rp.subScript = true;
        if (props.characterSpacing) rp.characterSpacing = props.characterSpacing;
        if (props.allCaps) rp.allCaps = true;
        if (props.smallCaps) rp.smallCaps = true;
        if (props.style) rp.style = props.style;
        return rp;
    }

    function _makeRun(content, props) {
        const rp = _runProps(props || {});
        if (typeof content === 'string') {
            rp.text = content;
        } else if (Array.isArray(content)) {
            rp.children = content;
        }
        return new TextRun(rp);
    }

    function _splitEmojiRuns(str, props) {
        _EMOJI_RE.lastIndex = 0;
        if (!_EMOJI_RE.test(str)) return [_makeRun(str, props)];
        _EMOJI_RE.lastIndex = 0;
        const runs = [];
        let last = 0;
        let m;
        while ((m = _EMOJI_RE.exec(str)) !== null) {
            if (m.index > last) runs.push(_makeRun(str.slice(last, m.index), props));
            runs.push(_makeRun(m[0], { ...props, font: _EMOJI_FONT }));
            last = _EMOJI_RE.lastIndex;
        }
        if (last < str.length) runs.push(_makeRun(str.slice(last), props));
        return runs;
    }

    function text(content, props) {
        if (typeof content === 'string') {
            if (content.includes('$')) {
                const parts = _splitInlineMath(content, props || {});
                return parts.length === 1 ? parts[0] : parts;
            }
            const runs = _splitEmojiRuns(content, props || {});
            return runs.length === 1 ? runs[0] : runs;
        }
        return _makeRun(content, props || {});
    }
    function bold(content, extra) {
        // LLM 常误用 h.bold('标题', '后续文本') 来拼接粗体+普通文本，
        // 第二参数实际应为样式对象。当检测到字符串时自动降级为拼接。
        if (typeof extra === 'string') {
            return [text(content, { bold: true }), text(extra)];
        }
        return text(content, { bold: true, ...extra });
    }
    function italic(content, extra) {
        if (typeof extra === 'string') {
            return [text(content, { italic: true }), text(extra)];
        }
        return text(content, { italic: true, ...extra });
    }

    // ── Content coercion ──────────────────────────────────────────

    const RUN_KEYS = new Set([
        'bold', 'italic', 'italics', 'underline', 'strike', 'color',
        'size', 'font', 'highlight', 'superScript', 'subScript',
        'characterSpacing', 'allCaps', 'smallCaps',
    ]);

    function _extractRunProps(opts) {
        const rp = {};
        for (const k of RUN_KEYS) {
            if (opts[k] !== undefined) rp[k] = opts[k];
        }
        return rp;
    }

    const _INLINE_MATH_RE = /\$([^$]+?)\$/g;
    const _INLINE_CITE_RE = /\[@([^\]]+?)\]/g;
    const _formula = require(path.join(__dirname, 'formula'));
    let _activeRefTracker = null;

    function _splitInlineMath(str, runProps) {
        if (!str || !str.includes('$')) return _splitEmojiRuns(str, runProps);
        const parts = [];
        let last = 0;
        let m;
        _INLINE_MATH_RE.lastIndex = 0;
        while ((m = _INLINE_MATH_RE.exec(str)) !== null) {
            if (m.index > last) {
                parts.push(..._splitEmojiRuns(str.slice(last, m.index), runProps));
            }
            parts.push(_formula.createMath(m[1]));
            last = _INLINE_MATH_RE.lastIndex;
        }
        if (last < str.length) parts.push(..._splitEmojiRuns(str.slice(last), runProps));
        return parts;
    }

    const _BARE_CITE_RE = /\[([a-zA-Z][a-zA-Z0-9_]*(?:\d{4}[a-z]?)?(?:,[a-zA-Z][a-zA-Z0-9_]*(?:\d{4}[a-z]?)?)*)\]/g;
    const _NUMERIC_CITE_RE = /\[(\d+(?:\s*[-,，]\s*\d+)*)\]/g;

    function _numericCiteToSuperscript(str, runProps) {
        if (!_activeRefTracker || !str) return _splitInlineMath(str, runProps);
        _NUMERIC_CITE_RE.lastIndex = 0;
        if (!_NUMERIC_CITE_RE.test(str)) return _splitInlineMath(str, runProps);
        _NUMERIC_CITE_RE.lastIndex = 0;
        const supSize = _int(_activeRefTracker._supSize) || _int(cfg.sizes.ref) || _int(cfg.sizes.small) || 18;
        const refFont = _activeRefTracker._refFont || cfg.fonts.english || cfg.fonts.body;
        const parts = [];
        let last = 0;
        let m;
        while ((m = _NUMERIC_CITE_RE.exec(str)) !== null) {
            if (/^\s*$/.test(str.slice(0, m.index))) {
                parts.push(..._splitInlineMath(m[0], runProps));
                last = _NUMERIC_CITE_RE.lastIndex;
                continue;
            }
            if (m.index > last) {
                parts.push(..._splitInlineMath(str.slice(last, m.index), runProps));
            }
            parts.push(new TextRun({
                text: m[0],
                superScript: true,
                font: refFont,
                size: supSize,
            }));
            last = _NUMERIC_CITE_RE.lastIndex;
        }
        if (last < str.length) {
            parts.push(..._splitInlineMath(str.slice(last), runProps));
        }
        return parts;
    }

    function _splitInlineCite(str, runProps) {
        if (!_activeRefTracker || !str || !str.includes('[@')) {
            if (_activeRefTracker && str && _BARE_CITE_RE.test(str)) {
                _BARE_CITE_RE.lastIndex = 0;
                let bm;
                while ((bm = _BARE_CITE_RE.exec(str)) !== null) {
                    if (!/^\[\d+\]$/.test(bm[0]) && !/^\[[A-Z]\]$/.test(bm[0])) {
                        console.warn(
                            `[docx-helpers] 警告: 疑似引用缺少 @ 前缀: "${bm[0]}" → 应为 "[@${bm[1]}]"`
                        );
                    }
                }
            }
            return _numericCiteToSuperscript(str, runProps);
        }
        const parts = [];
        let last = 0;
        let m;
        _INLINE_CITE_RE.lastIndex = 0;
        while ((m = _INLINE_CITE_RE.exec(str)) !== null) {
            if (m.index > last) {
                parts.push(..._splitInlineMath(str.slice(last, m.index), runProps));
            }
            const keys = m[1].split(/[,;，；]/).map(k => k.trim().replace(/^@/, '')).filter(Boolean);
            parts.push(_activeRefTracker.cite(...keys));
            last = _INLINE_CITE_RE.lastIndex;
        }
        if (last < str.length) {
            parts.push(..._splitInlineMath(str.slice(last), runProps));
        }
        return parts;
    }

    function _toChildren(content, defaultRunProps) {
        if (content == null) return [];
        if (typeof content === 'string') return _splitInlineCite(content, defaultRunProps || {});
        if (content instanceof TextRun || content instanceof ImageRun) return [content];
        if (content instanceof Paragraph) {
            return content.root && content.root.length ? content.root.filter(c => c instanceof ImageRun || c instanceof TextRun) : [];
        }
        if (Array.isArray(content)) {
            return content.flatMap(item => {
                if (typeof item === 'string') return _splitInlineCite(item, defaultRunProps || {});
                if (item instanceof Paragraph) {
                    return item.root && item.root.length ? item.root.filter(c => c instanceof ImageRun || c instanceof TextRun) : [];
                }
                if (Array.isArray(item)) return item;
                return [item];
            });
        }
        return [content];
    }

    // ── Paragraph helpers ─────────────────────────────────────────

    function p(content, opts) {
        opts = opts || {};
        if (content instanceof Paragraph) {
            return _spreadableBlock(content);
        }
        if (content instanceof Table) {
            return _spreadableBlock(content);
        }
        const pp = {};
        if (opts.align) pp.alignment = ALIGN[opts.align] || opts.align;
        if (opts.spacing) pp.spacing = _intObj(opts.spacing);
        if (opts.indent) pp.indent = _intObj(opts.indent);
        if (opts.heading) pp.heading = opts.heading;
        if (opts.numbering) pp.numbering = opts.numbering;
        if (opts.pageBreakBefore) pp.pageBreakBefore = true;
        if (opts.shading) pp.shading = opts.shading;
        if (opts.border) pp.border = opts.border;
        if (opts.tabStops) pp.tabStops = opts.tabStops;
        if (opts.contextualSpacing !== undefined) pp.contextualSpacing = opts.contextualSpacing;
        if (opts.keepNext) pp.keepNext = true;
        if (opts.keepLines) pp.keepLines = true;
        if (opts.widowControl !== undefined) pp.widowControl = opts.widowControl;
        if (!pp.spacing && !opts.heading && !opts.numbering) {
            pp.spacing = { ...cfg.spacing.body };
        }
        const _align = opts.align && (ALIGN[opts.align] || opts.align);
        if (!pp.indent && !opts.heading && !opts.numbering && cfg.indent
            && _align !== AlignmentType.CENTER && _align !== AlignmentType.RIGHT) {
            pp.indent = { firstLine: _int(cfg.indent) };
        }

        pp.children = _toChildren(content, _extractRunProps(opts));
        return _spreadableBlock(new Paragraph(pp));
    }

    const _headingRegistry = [];

    function _heading(level, content, opts) {
        opts = opts || {};
        const headings = [null, HeadingLevel.HEADING_1, HeadingLevel.HEADING_2, HeadingLevel.HEADING_3];
        const sizes = [null, cfg.sizes.h1, cfg.sizes.h2, cfg.sizes.h3];

        const textStr = typeof content === 'string' ? content : String(content);
        _headingRegistry.push({
            text: textStr,
            level,
            bookmark: opts.bookmark || null,
        });

        if (opts.bookmark) {
            const runProps = {
                bold: true,
                font: opts.font || cfg.fonts.heading,
                size: _int(opts.size) || sizes[level],
                color: opts.color || cfg.colors.primary,
            };
            const children = _toChildren(content, runProps);
            const bm = new Bookmark({ id: opts.bookmark, children });
            return _spreadableBlock(new Paragraph({
                heading: headings[level],
                spacing: _intObj(opts.spacing) || { ...cfg.spacing.heading },
                keepNext: true,
                keepLines: true,
                children: [bm],
            }));
        }

        return p(content, {
            heading: headings[level],
            spacing: { ...cfg.spacing.heading },
            bold: true,
            font: cfg.fonts.heading,
            size: sizes[level],
            color: cfg.colors.primary,
            keepNext: true,
            keepLines: true,
            ...opts,
        });
    }

    function h1(content, opts) { return _heading(1, content, opts); }
    function h2(content, opts) { return _heading(2, content, opts); }
    function h3(content, opts) { return _heading(3, content, opts); }

    function bullet(content, opts) {
        opts = opts || {};
        const level = opts.level || 0;
        return p(content, {
            numbering: { reference: BULLET_REF, level },
            spacing: { after: 80 },
            ...opts,
        });
    }

    function numbered(content, opts) {
        opts = opts || {};
        const level = opts.level || 0;
        const ref = opts.ref || NUMBER_REF;
        _usedNumberingRefs.add(ref);
        return p(content, {
            numbering: { reference: ref, level },
            spacing: { after: 80 },
            ...opts,
        });
    }

    // ── Layout helpers ────────────────────────────────────────────

    function pageBreak_() {
        return _spreadableBlock(new Paragraph({ children: [new PageBreak()] }));
    }

    function spacer(height) {
        const p = new Paragraph({
            spacing: { before: _int(height) || 400, line: 20, lineRule: LineRuleType.EXACT },
        });
        p._isSpacer = true;
        p._spacerHeight = _int(height) || 400;
        return _spreadableBlock(p);
    }

    function divider(color, size) {
        // Tolerate LLM-common mis-calls without surfacing the cryptic
        // "Invalid value '[object Object]' specified. Must be an integer."
        // from docx-js. Accepted shapes (all map to (color: string, size: int)):
        //   divider('FFFFFF', 6)               // canonical
        //   divider('FFFFFF', { size: 6 })     // size obj  (observed 4×)
        //   divider({ color, size })           // opts obj  (observed 1×)
        //   divider({ color }, { size })       // mixed     (defensive)
        if (color && typeof color === 'object' && !Array.isArray(color)) {
            const o = color;
            color = o.color || o.fill || undefined;
            if (size === undefined || (size && typeof size === 'object')) {
                size = (size && typeof size === 'object' ? size : o).size
                    || (size && typeof size === 'object' ? size : o).width
                    || o.size || o.width;
            }
        }
        if (size && typeof size === 'object') {
            size = size.size || size.width;
        }
        return _spreadableBlock(new Paragraph({
            border: { bottom: { style: BorderStyle.SINGLE, size: _int(size) || 6, color: color || cfg.colors.border } },
            spacing: { after: 200 },
        }));
    }

    const _floatTblpPrCache = _columnCount > 1 ? _buildFloatTblpPr() : null;

    function _buildFloatTblpPr() {
        const ref = new Table({
            float: {
                horizontalAnchor: TableAnchorType.MARGIN,
                verticalAnchor: TableAnchorType.TEXT,
                relativeHorizontalPosition: RelativeHorizontalPosition.CENTER,
                overlap: OverlapType.NEVER,
                topFromText: 120,
                bottomFromText: 120,
            },
            width: { size: 1, type: WidthType.DXA },
            rows: [new TableRow({ children: [new TableCell({ children: [new Paragraph('')] })] })],
        });
        for (const child of ref.root) {
            if (child && child.rootKey === 'w:tblPr') {
                for (const sub of child.root) {
                    if (sub && sub.rootKey === 'w:tblpPr') return sub;
                }
            }
        }
        return null;
    }

    function _injectFloat(tbl) {
        if (!_floatTblpPrCache) return;
        for (const child of tbl.root) {
            if (child && child.rootKey === 'w:tblPr') {
                const hasPr = child.root.some(s => s && s.rootKey === 'w:tblpPr');
                if (!hasPr) child.root.unshift(_floatTblpPrCache);
                return;
            }
        }
    }

    function _scaleTableToFullWidth(tbl) {
        for (const child of tbl.root) {
            if (child && child.rootKey === 'w:tblPr') {
                for (const sub of child.root) {
                    if (sub && sub.rootKey === 'w:tblW') {
                        for (const attr of sub.root) {
                            if (attr && attr.rootKey === '_attr' && attr.root && attr.root.size) {
                                attr.root.size.value = _fullContentWidth;
                            }
                        }
                    }
                }
            }
        }
    }

    function _getTableColumnCount(tbl) {
        for (const child of tbl.root) {
            if (child instanceof TableRow) {
                let count = 0;
                for (const cell of child.root) {
                    if (cell instanceof TableCell) count++;
                }
                return count;
            }
        }
        return 0;
    }

    function _mergeCaptionIntoTable(captionPara, tbl, position) {
        const colCount = _getTableColumnCount(tbl);
        if (colCount < 1) return;
        const NONE = { style: BorderStyle.NIL, size: 0 };
        const captionCell = new TableCell({
            children: [captionPara],
            columnSpan: colCount,
            width: { size: _fullContentWidth, type: WidthType.DXA },
            borders: { top: NONE, bottom: NONE, left: NONE, right: NONE },
            margins: { top: 60, bottom: 60, left: 0, right: 0 },
        });
        const captionRow = new TableRow({ children: [captionCell] });
        if (position === 'before') {
            const insertIdx = tbl.root.findIndex(c => c instanceof TableRow);
            if (insertIdx >= 0) tbl.root.splice(insertIdx, 0, captionRow);
            else tbl.root.push(captionRow);
        } else {
            tbl.root.push(captionRow);
        }
    }

    function _scaleImageToFullWidth(para) {
        const scale = _fullContentWidth / _columnContentWidth;
        const pageContentHeight = cfg.page.height - cfg.page.margins.top - cfg.page.margins.bottom;
        const maxHeightEMU = Math.round(pageContentHeight * 0.3 * 635);
        function walk(node) {
            if (!node || !node.root) return;
            if (node.rootKey === 'wp:extent') {
                for (const attr of node.root) {
                    if (attr && attr.rootKey === '_attr' && attr.root) {
                        let newCx = Math.round((attr.root.x ? attr.root.x.value : 0) * scale);
                        let newCy = Math.round((attr.root.y ? attr.root.y.value : 0) * scale);
                        if (newCy > maxHeightEMU && newCy > 0) {
                            const hScale = maxHeightEMU / newCy;
                            newCx = Math.round(newCx * hScale);
                            newCy = maxHeightEMU;
                        }
                        if (attr.root.x) attr.root.x.value = newCx;
                        if (attr.root.y) attr.root.y.value = newCy;
                    }
                }
                return;
            }
            if (Array.isArray(node.root)) {
                for (const child of node.root) walk(child);
            }
        }
        walk(para);
    }

    function _wrapInFloatTable(paragraphs) {
        for (const p_ of paragraphs) _scaleImageToFullWidth(p_);
        const NONE = { style: BorderStyle.NIL, size: 0 };
        const noBorders = { top: NONE, bottom: NONE, left: NONE, right: NONE };
        const row = new TableRow({
            children: [new TableCell({
                children: paragraphs,
                width: { size: _fullContentWidth, type: WidthType.DXA },
                borders: noBorders,
                margins: { top: 0, bottom: 0, left: 0, right: 0 },
            })],
        });
        const tbl = new Table({
            float: {
                horizontalAnchor: TableAnchorType.MARGIN,
                verticalAnchor: TableAnchorType.TEXT,
                relativeHorizontalPosition: RelativeHorizontalPosition.CENTER,
                overlap: OverlapType.NEVER,
                topFromText: 120,
                bottomFromText: 120,
            },
            width: { size: _fullContentWidth, type: WidthType.DXA },
            columnWidths: [_fullContentWidth],
            rows: [row],
            borders: noBorders,
        });
        return tbl;
    }

    function fullWidth(...args) {
        const savedWidth = contentWidth;
        contentWidth = _fullContentWidth;
        let items;
        try {
            items = [];
            for (const a of args) {
                if (typeof a === 'function') {
                    const result = a(_fullContentWidth);
                    if (Array.isArray(result)) items.push(...result.flat(Infinity));
                    else if (result != null) items.push(result);
                } else if (Array.isArray(a)) {
                    items.push(...a.flat(Infinity));
                } else if (a != null) {
                    items.push(a);
                }
            }
        } finally {
            contentWidth = savedWidth;
        }

        if (_columnCount <= 1) return items;

        let result;
        const hasTable = items.some(it => it instanceof Table);
        if (!hasTable) {
            result = [_wrapInFloatTable(items)];
        } else {
            const toRemove = new Set();
            for (let i = 0; i < items.length; i++) {
                if (!(items[i] instanceof Table)) continue;
                const tbl = items[i];
                _injectFloat(tbl);
                _scaleTableToFullWidth(tbl);

                if (i > 0 && items[i - 1] instanceof Paragraph) {
                    _mergeCaptionIntoTable(items[i - 1], tbl, 'before');
                    toRemove.add(i - 1);
                }
                if (i + 1 < items.length && items[i + 1] instanceof Paragraph) {
                    _mergeCaptionIntoTable(items[i + 1], tbl, 'after');
                    toRemove.add(i + 1);
                }
            }
            result = toRemove.size > 0 ? items.filter((_, idx) => !toRemove.has(idx)) : items;
        }

        result.push(new Paragraph({ spacing: { before: 0, after: 0, line: 20 } }));
        return result;
    }

    // ── Image helper ──────────────────────────────────────────────

    function _readImageSize(buf) {
        // PNG: bytes 16-23 contain width(4) + height(4) in IHDR chunk
        if (buf.length >= 24 && buf[0] === 0x89 && buf[1] === 0x50) {
            return { w: buf.readUInt32BE(16), h: buf.readUInt32BE(20) };
        }
        // JPEG: scan for SOFn marker (0xFF 0xC0..0xCF, excluding 0xC4/0xC8/0xCC)
        if (buf.length >= 2 && buf[0] === 0xFF && buf[1] === 0xD8) {
            let off = 2;
            while (off + 9 < buf.length) {
                if (buf[off] !== 0xFF) { off++; continue; }
                const marker = buf[off + 1];
                if (marker >= 0xC0 && marker <= 0xCF && marker !== 0xC4 && marker !== 0xC8 && marker !== 0xCC) {
                    return { w: buf.readUInt16BE(off + 7), h: buf.readUInt16BE(off + 5) };
                }
                off += 2 + buf.readUInt16BE(off + 2);
            }
        }
        return null;
    }

    function img(filePath, opts) {
        opts = opts || {};
        if (!filePath || typeof filePath !== 'string') {
            throw new Error('h.img() 第一个参数必须是图片文件路径（字符串）');
        }
        if (!fs.existsSync(filePath)) {
            // Fuzzy fallback: charts saved by upstream tools land at a
            // slightly different folder than the JS expects (e.g. workspace
            // root vs ./output subdir). Try common siblings before bailing.
            const base = path.basename(filePath);
            const tried = new Set([filePath]);
            const candidates = [
                path.join(process.cwd(), base),
                path.join(path.dirname(filePath), '..', base),
                path.join(path.dirname(filePath), 'output', base),
                path.join(path.dirname(path.dirname(filePath)), base),
            ];
            let resolved = null;
            for (const c of candidates) {
                const abs = path.resolve(c);
                if (tried.has(abs)) continue;
                tried.add(abs);
                if (fs.existsSync(abs) && fs.statSync(abs).isFile()) {
                    resolved = abs;
                    break;
                }
            }
            if (resolved) {
                filePath = resolved;
            } else {
                throw new Error(
                    `h.img() 图片文件不存在: ${filePath}\n` +
                    `  已尝试候选: ${candidates.map(c => path.resolve(c)).join(' | ')}\n` +
                    `  请确认图表实际生成位置（常见错误: 把图保存在工作区根目录，但脚本里写了 output/xxx.png）。`
                );
            }
        }
        const ext = (filePath.split('.').pop() || 'png').toLowerCase();
        const data = fs.readFileSync(filePath);
        const realSize = _readImageSize(data);
        let w, h_;
        const hasW = opts.width != null;
        const hasH = opts.height != null;
        if (hasW && hasH) {
            w = _int(opts.width);
            h_ = _int(opts.height);
        } else if (hasW && realSize) {
            w = _int(opts.width);
            h_ = Math.round(w * realSize.h / realSize.w);
        } else if (hasH && realSize) {
            h_ = _int(opts.height);
            w = Math.round(h_ * realSize.w / realSize.h);
        } else if (realSize) {
            const ratio = realSize.h / realSize.w;
            w = Math.round(contentWidth / 15);
            h_ = Math.round(w * ratio);
        } else {
            w = _int(opts.width) || 400;
            h_ = _int(opts.height) || 300;
        }
        const maxPx = Math.round(contentWidth / 15);
        if (w > maxPx) {
            const scale = maxPx / w;
            h_ = Math.round(h_ * scale);
            w = maxPx;
        }
        const maxHPx = Math.round((cfg.page.height - cfg.page.margins.top - cfg.page.margins.bottom) / 15 * 0.85);
        if (h_ > maxHPx && maxHPx > 0) {
            const hScale = maxHPx / h_;
            w = Math.round(w * hScale);
            h_ = maxHPx;
        }
        const imgProps = {
            type: ext === 'jpg' ? 'jpeg' : ext,
            data,
            transformation: { width: w, height: h_ },
        };
        if (opts.altText) imgProps.altText = opts.altText;
        if (opts.floating) imgProps.floating = opts.floating;
        const run = new ImageRun(imgProps);
        if (opts._raw) return run;
        return _spreadableBlock(new Paragraph({
            alignment: ALIGN[opts.align] || opts.align || AlignmentType.CENTER,
            children: [run],
            spacing: _intObj(opts.spacing) || {},
        }));
    }

    // ── Link helpers ──────────────────────────────────────────────

    function link(displayText, url) {
        return new ExternalHyperlink({
            children: [new TextRun({ text: displayText, style: 'Hyperlink', color: cfg.colors.primary })],
            link: url,
        });
    }

    // ── Header / Footer / PageNumber ─────────────────────────────

    function header(content, opts) {
        opts = opts || {};
        const defaultProps = { size: 18, color: '999999', align: 'center' };
        const children = [p(content, { ...defaultProps, ...opts })];
        return new Header({ children });
    }

    function footer(content, opts) {
        opts = opts || {};
        if (content == null) {
            return new Footer({ children: [p([
                text('\u2014 ', { size: 18, color: '999999', _noDefaultFont: true }),
                text([PageNumber.CURRENT], { size: 18, color: '999999', _noDefaultFont: true }),
                text(' \u2014', { size: 18, color: '999999', _noDefaultFont: true }),
            ], { align: 'center' })] });
        }
        if (typeof content === 'string' && content.indexOf('CURRENT') !== -1) {
            console.warn('[docx-helpers] footer: pageNum() 被字符串拼接为 "CURRENT" → 已自动替换为页码字段');
            const parts = content.split('CURRENT');
            const runs = [];
            for (let i = 0; i < parts.length; i++) {
                if (parts[i]) runs.push(text(parts[i], { size: 18, color: '999999', _noDefaultFont: true }));
                if (i < parts.length - 1) runs.push(text([PageNumber.CURRENT], { size: 18, color: '999999', _noDefaultFont: true }));
            }
            return new Footer({ children: [p(runs, { align: 'center', ...(opts || {}) })] });
        }
        const defaultProps = { size: 18, color: '999999', align: 'center' };
        return new Footer({ children: [p(content, { ...defaultProps, ...opts })] });
    }

    function pageNum() {
        return [PageNumber.CURRENT];
    }

    function headerFooter(headerContent, footerContent, opts) {
        opts = opts || {};
        return {
            headers: { default: header(headerContent, opts.header || {}) },
            footers: { default: typeof footerContent === 'undefined'
                ? footer()
                : footer(footerContent, opts.footer || {}) },
        };
    }

    // ── Cover background ──────────────────────────────────────────

    function coverBg(imgPath, opts) {
        opts = opts || {};
        const pxW = Math.round(cfg.page.width / 15);
        const pxH = Math.round(cfg.page.height / 15);
        return img(imgPath, {
            width: opts.width || pxW,
            height: opts.height || pxH,
            floating: {
                horizontalPosition: { relative: HorizontalPositionRelativeFrom.PAGE, offset: 0 },
                verticalPosition: { relative: VerticalPositionRelativeFrom.PAGE, offset: 0 },
                behindDocument: true,
            },
        });
    }

    // ── Table of Contents ─────────────────────────────────────────

    let _tocRef = null;
    let _tocOpts = null;

    function toc(opts) {
        opts = opts || {};
        _tocOpts = opts;
        const noIndent = { indent: { firstLine: 0 } };
        if (opts.cachedEntries) {
            const tocObj = new TableOfContents(opts.title || '\u76EE\u5F55', {
                hyperlink: opts.hyperlink !== false,
                headingStyleRange: opts.headingStyleRange || '1-3',
                cachedEntries: opts.cachedEntries,
            });
            return p([tocObj], noIndent);
        }
        const tocObj = new TableOfContents(opts.title || '\u76EE\u5F55', {
            hyperlink: opts.hyperlink !== false,
            headingStyleRange: opts.headingStyleRange || '1-3',
        });
        const tocPara = p([tocObj], noIndent);
        _tocRef = tocPara;
        return tocPara;
    }

    // ── Bookmark helper ───────────────────────────────────────────

    function bookmark(id, content) {
        const children = _toChildren(content, { bold: true });
        return new Bookmark({ id, children });
    }

    // ── Vertical merge constant ───────────────────────────────────

    const MERGE = {
        START: VerticalMergeType.RESTART,
        CONTINUE: VerticalMergeType.CONTINUE,
    };

    // ── Table helpers ─────────────────────────────────────────────

    const _cellSpacing = { after: 0, line: 320 };

    function _cellChildren(content) {
        if (content instanceof Paragraph) return [content];
        if (content instanceof Table) return [content];
        if (content instanceof TextRun) {
            return [new Paragraph({ spacing: _cellSpacing, children: [content] })];
        }
        if (Array.isArray(content)) {
            if (content.length > 0 && (content[0] instanceof Paragraph || content[0] instanceof Table)) {
                return content;
            }
            return [new Paragraph({ spacing: _cellSpacing, children: _toChildren(content) })];
        }
        if (typeof content === 'string') {
            return [new Paragraph({ spacing: _cellSpacing, children: _toChildren(content) })];
        }
        return [new Paragraph({ spacing: _cellSpacing, children: [new TextRun(String(content != null ? content : ''))] })];
    }

    function _isCellObject(content) {
        return typeof content === 'object' && content !== null &&
            !(content instanceof Paragraph) && !(content instanceof Table) &&
            !(content instanceof TextRun) && !Array.isArray(content);
    }

    function _normalizeCellObject(content) {
        if (!_isCellObject(content)) return content;
        const nested = (
            content.opts && typeof content.opts === 'object' && !Array.isArray(content.opts)
                ? content.opts
                : content.options && typeof content.options === 'object' && !Array.isArray(content.options)
                    ? content.options
                    : content.style && typeof content.style === 'object' && !Array.isArray(content.style)
                        ? content.style
                        : null
        );
        if (!nested) return content;
        const merged = { ...nested, ...content };
        delete merged.opts;
        delete merged.options;
        if (typeof content.style === 'object') delete merged.style;
        return merged;
    }

    function _rawCellSpan(content) {
        const c = _normalizeCellObject(content);
        if (!_isCellObject(c)) return 1;
        const span = c.columnSpan ?? c.colSpan ?? c.col_span;
        const n = parseInt(span, 10);
        return Number.isFinite(n) && n > 0 ? n : 1;
    }

    function _inferTableColumns(header, rows) {
        let maxCols = 0;
        const scan = (cells) => {
            if (!Array.isArray(cells)) return;
            let count = 0;
            for (const c of cells) count += _rawCellSpan(c);
            if (count > maxCols) maxCols = count;
        };
        scan(header);
        if (Array.isArray(rows)) rows.forEach(scan);
        return maxCols;
    }

    function table(spec) {
        spec = spec || {};
        const header = _firstDefined(spec, ['header', 'headers', '表头', '标题行', '头部']);
        const rows = _firstDefined(spec, ['rows', 'dataRows', 'data', '行', '数据行', '表格行', '内容']) || [];
        const {
            width: _width,
            headerColor,
            headerTextColor,
            altColor,
            borders = true,
            margins,
            noBorders,
            align,
        } = spec;
        const _widths = _firstDefined(spec, ['widths', 'columnWidths', 'column_widths', '列宽', '宽度']);
        const rowHeight = _firstDefined(spec, [
            'rowHeight', 'rowheight', 'row_height',
            'rowHeightTwips', 'row_height_twips',
            'tableRowHeight', 'trHeight', 'tr_height',
            'height',
        ]);
        const rowHeights = _firstDefined(spec, [
            'rowHeights', 'rowheights', 'row_heights',
            'rowHeightsTwips', 'row_heights_twips',
            'rowsHeight', 'rowsHeights', 'heights',
        ]);
        const heightRule = _firstDefined(spec, [
            'heightRule', 'height_rule',
            'rowHeightRule', 'row_height_rule',
            'hRule', 'h_rule', 'rule',
        ]) || HEIGHT_RULE_RAW.EXACT;

        let widths = _intArr(_widths || (Array.isArray(_width) ? _width : null));
        if (!widths || !Array.isArray(widths)) {
            const inferredCols = _inferTableColumns(header, rows);
            if (inferredCols > 0) {
                const colWidth = Math.max(1, Math.floor(contentWidth / inferredCols));
                widths = Array(inferredCols).fill(colWidth);
            }
        }
        if (!widths || !Array.isArray(widths)) {
            throw new Error(
                'h.table() 需要 widths 数组（列宽 DXA）。'
                + (spec.width ? ' 注意：不要传 width/columnWidths，请用 widths: [...]' : '')
            );
        }
        let totalWidth = widths.reduce((a, b) => a + b, 0);
        if (totalWidth > contentWidth && _columnCount > 1) {
            const testWidths = widths.map(w => Math.round(w * contentWidth / totalWidth));
            const minCol = Math.min(...testWidths);
            if (minCol < 700) {
                return fullWidth(() => table(spec));
            }
        }
        if (totalWidth > contentWidth) {
            const scale = contentWidth / totalWidth;
            widths = widths.map(w => Math.round(w * scale));
            totalWidth = widths.reduce((a, b) => a + b, 0);
            const diff = contentWidth - totalWidth;
            if (diff !== 0) widths[widths.length - 1] += diff;
            totalWidth = contentWidth;
        }

        // Auto-expand widths when header/rows have more columns than declared
        {
            let maxCols = 0;
            if (header && Array.isArray(header)) maxCols = header.length;
            for (const row of rows) {
                if (!Array.isArray(row)) continue;
                let cols = 0;
                for (const c of row) {
                    const span = (c && typeof c === 'object' && !Array.isArray(c) && c.columnSpan)
                        ? (parseInt(c.columnSpan, 10) || 1) : 1;
                    cols += span;
                }
                if (cols > maxCols) maxCols = cols;
            }
            if (maxCols > widths.length) {
                const avgWidth = widths.length > 0
                    ? Math.max(1, Math.round(totalWidth / widths.length))
                    : Math.max(1, Math.round(contentWidth / maxCols));
                while (widths.length < maxCols) widths.push(avgWidth);
                totalWidth = widths.reduce((a, b) => a + b, 0);
            }
        }

        const stdBorder = noBorders
            ? { style: BorderStyle.NIL }
            : borders === true
                ? { style: BorderStyle.SINGLE, size: 1, color: cfg.colors.border }
                : borders || { style: BorderStyle.NIL };
        const defaultBorders = { top: stdBorder, bottom: stdBorder, left: stdBorder, right: stdBorder };
        const defaultMargins = _intObj(margins) || { top: 100, bottom: 100, left: 120, right: 120 };

        function _fitWidthsToContent() {
            if (!Array.isArray(widths) || widths.length === 0) return;
            if (totalWidth <= contentWidth) return;
            const scale = contentWidth / totalWidth;
            widths = widths.map(w => Math.max(1, Math.round(w * scale)));
            totalWidth = widths.reduce((a, b) => a + b, 0);
            const diff = contentWidth - totalWidth;
            if (diff !== 0) {
                const last = widths.length - 1;
                widths[last] = Math.max(1, widths[last] + diff);
                totalWidth = widths.reduce((a, b) => a + b, 0);
            }
        }

        function _coerceSpan(value, context) {
            if (value == null) return 1;
            const span = _int(value);
            if (!Number.isInteger(span) || span < 1) {
                throw new Error(`h.table() ${context} 的 columnSpan 必须是 >= 1 的整数`);
            }
            return span;
        }

        function _spanWidth(startCol, span, context) {
            if (startCol + span > widths.length) {
                throw new Error(
                    `h.table() ${context} 覆盖到第 ${startCol + span} 列，但表格只声明了 ${widths.length} 列。\n` +
                    `  常见原因: columnSpan: N 表示当前单元格独占 N 列，被合并的列不要再放 cell。\n` +
                    `    ✘ [{text:'采购原因', columnSpan:3}, '', '', '']   // 错: 3 列合并里又写了 3 个占位\n` +
                    `    ✓ [{text:'采购原因', columnSpan:3}, '']           // 对: 合并占前 3 列，再放一个 cell 占第 4 列\n` +
                    `  每行 cells 数量 (含 columnSpan) 加和必须 = widths.length (${widths.length})。`
                );
            }
            return widths.slice(startCol, startCol + span).reduce((a, b) => a + b, 0);
        }

        function _cellSpan(content) {
            const c = _normalizeCellObject(content);
            if (_isCellObject(c)) {
                return _coerceSpan(c.columnSpan ?? c.colSpan ?? c.col_span, '单元格');
            }
            return 1;
        }

        function makeCell(content, colIdx, cellOpts) {
            cellOpts = cellOpts || {};
            content = _normalizeCellObject(content);
            const isCellObject = _isCellObject(content);
            const columnSpan = isCellObject ? _cellSpan(content) : _coerceSpan(cellOpts.columnSpan, '单元格');
            const cp = {
                width: { size: _spanWidth(colIdx, columnSpan, '单元格'), type: WidthType.DXA },
                borders: cellOpts.borders || defaultBorders,
                margins: cellOpts.margins || defaultMargins,
            };
            if (cellOpts.fill) cp.shading = { fill: cellOpts.fill, type: ShadingType.CLEAR };
            if (columnSpan > 1) cp.columnSpan = columnSpan;
            if (cellOpts.verticalMerge) cp.verticalMerge = cellOpts.verticalMerge;
            const cellOptVerticalAlign = _firstDefined(cellOpts, ['verticalAlign', 'vertical_align', 'valign', 'vAlign']);
            const cellOptTextDirection = _firstDefined(cellOpts, ['textDirection', 'text_direction', 'textDir', 'direction', 'orientation']);
            if (cellOptVerticalAlign) cp.verticalAlign = _mapVerticalAlign(cellOptVerticalAlign);
            if (cellOptTextDirection) cp.textDirection = _mapTextDirection(cellOptTextDirection);

            if (isCellObject) {
                const {
                    text: t,
                    children: ch,
                    fill,
                    columnSpan: _ignoredColumnSpan,
                    colSpan: _ignoredColSpan,
                    col_span: _ignoredColSpanSnake,
                    verticalMerge,
                    verticalAlign: va,
                    vertical_align,
                    valign,
                    vAlign,
                    textDirection,
                    text_direction,
                    textDir,
                    direction,
                    orientation,
                    align,
                    borders: contentBorders,
                    margins: contentMargins,
                    opts: _ignoredOpts,
                    options: _ignoredOptions,
                    ...runProps
                } = content;
                if (contentBorders) cp.borders = contentBorders;
                if (contentMargins) cp.margins = _intObj(contentMargins);
                if (fill) cp.shading = { fill, type: ShadingType.CLEAR };
                if (verticalMerge) cp.verticalMerge = verticalMerge;
                const contentVerticalAlign = va ?? vertical_align ?? valign ?? vAlign;
                const contentTextDirection = textDirection ?? text_direction ?? textDir ?? direction ?? orientation;
                if (contentVerticalAlign) cp.verticalAlign = _mapVerticalAlign(contentVerticalAlign);
                if (contentTextDirection) cp.textDirection = _mapTextDirection(contentTextDirection);
                const cellText = ch ?? t ?? '';
                if (Object.keys(runProps).length > 0 && typeof cellText === 'string') {
                    cp.children = [new Paragraph({
                        spacing: { after: 0 },
                        alignment: align ? (ALIGN[align] || align) : undefined,
                        children: [_makeRun(cellText, runProps)],
                    })];
                } else if (align && (typeof cellText === 'string' || Array.isArray(cellText) || cellText instanceof TextRun)) {
                    cp.children = [new Paragraph({
                        spacing: _cellSpacing,
                        alignment: ALIGN[align] || align,
                        children: _toChildren(cellText, runProps),
                    })];
                } else {
                    cp.children = _cellChildren(cellText);
                }
            } else {
                cp.children = _cellChildren(content);
            }
            return new TableCell(cp);
        }

        function _normalizeRowHeight(value, rule) {
            if (value == null) return null;
            function _rowHeightValue(v) {
                if (typeof v === 'string') {
                    const key = v.trim().toLowerCase();
                    if (key === 'page' || key === 'full' || key === 'fullpage' ||
                        key === 'content' || key === 'contentheight' ||
                        key === 'usable' || key === 'available') {
                        return _int(cfg.page.height - cfg.page.margins.top - cfg.page.margins.bottom);
                    }
                }
                return _int(v);
            }
            if (typeof value === 'object' && !Array.isArray(value)) {
                const h = _rowHeightValue(value.value ?? value.size ?? value.height);
                if (h == null) return null;
                return {
                    value: h,
                    rule: _mapRowHeightRule(
                        value.rule || value.heightRule || value.height_rule ||
                        value.rowHeightRule || value.row_height_rule || rule
                    ) || HEIGHT_RULE_RAW.EXACT,
                };
            }
            return {
                value: _rowHeightValue(value),
                rule: _mapRowHeightRule(rule) || HEIGHT_RULE_RAW.EXACT,
            };
        }

        function makeRow(cells, rowOpts) {
            rowOpts = rowOpts || {};
            const label = rowOpts.label || '某一行';
            if (!Array.isArray(cells)) {
                const got = cells === null ? 'null'
                    : Array.isArray(cells) ? 'array'
                    : typeof cells;
                const objHint = (got === 'object')
                    ? '\n    你可能把一个单元格对象 ({text:..., columnSpan:...}) 当成一整行了 — 再外面包一层 [] 即可。'
                    : '';
                throw new Error(
                    `h.table() ${label} 必须是数组 [cell, cell, …]，当前是 ${got}。` + objHint
                );
            }
            let colIdx = 0;
            const children = cells.map(c => {
                const cell = makeCell(c, colIdx, { fill: rowOpts.fill });
                colIdx += _cellSpan(c);
                return cell;
            });
            if (colIdx < widths.length) {
                if (cells.length === 0) {
                    throw new Error(
                        `h.table() ${label} cells 为空数组 — 如果表头不需要，直接不传 header，不要写 header: []。`
                    );
                }
                while (colIdx < widths.length) {
                    children.push(makeCell('', colIdx, { fill: rowOpts.fill }));
                    colIdx++;
                }
            }
            const rowProps = { children };
            const height = _normalizeRowHeight(rowOpts.height, rowOpts.heightRule);
            if (height) rowProps.height = height;
            return new TableRow(rowProps);
        }

        // 预扫描 header + 所有 data rows，找到实际最大列数，
        // 若超过 widths 声明的列数则自动扩展 widths（避免运行时 throw）。
        {
            let maxCols = widths.length;
            const _scanCols = (cells) => {
                if (!Array.isArray(cells)) return;
                let n = 0;
                cells.forEach(c => { n += _cellSpan(c); });
                if (n > maxCols) maxCols = n;
            };
            if (header && Array.isArray(header)) _scanCols(header);
            rows.forEach(r => _scanCols(r));
            if (maxCols > widths.length) {
                const avgW = widths.length > 0
                    ? Math.max(1, Math.round(totalWidth / widths.length))
                    : Math.max(1, Math.round(contentWidth / maxCols));
                while (widths.length < maxCols) widths.push(avgW);
                totalWidth = widths.reduce((a, b) => a + b, 0);
            }
        }

        // 列数自动扩展后，兜底再压回可用宽度，避免总宽随列数线性膨胀。
        _fitWidthsToContent();

        const allRows = [];
        const getRowHeight = (rowIdx) => Array.isArray(rowHeights)
            ? rowHeights[rowIdx]
            : rowHeight;

        if (header && Array.isArray(header) && header.length > 0) {
            const hColor = headerTextColor || (headerColor ? cfg.colors.white : cfg.colors.text);
            const hCells = header.map(h =>
                typeof h === 'string'
                    ? { text: h, bold: true, color: hColor, align: 'center' }
                    : { bold: true, color: hColor, align: 'center', ...h }
            );
            allRows.push(makeRow(hCells, {
                fill: headerColor,
                label: '表头行',
                height: getRowHeight(allRows.length),
                heightRule,
            }));
        }

        rows.forEach((row, ri) => {
            const fill = altColor && ri % 2 === 1 ? altColor : undefined;
            allRows.push(makeRow(row, {
                fill,
                label: `数据行 ${ri + 1}`,
                height: getRowHeight(allRows.length),
                heightRule,
            }));
        });

        return _spreadableBlock(new Table({
            width: { size: totalWidth, type: WidthType.DXA },
            columnWidths: widths,
            rows: allRows,
            alignment: ALIGN[align] || align || AlignmentType.CENTER,
        }));
    }

    // ── Cover page spacer auto-shrink ────────────────────────────

    function _shrinkCoverSpacers(children, availableTwips) {
        let spacerTotal = 0;
        const spacerIndices = [];
        let contentHeight = 0;
        for (let i = 0; i < children.length; i++) {
            const child = children[i];
            if (child._isSpacer) {
                spacerTotal += child._spacerHeight;
                spacerIndices.push(i);
            } else if (child instanceof Table) {
                let rowCount = 0;
                for (const c of child.root) { if (c instanceof TableRow) rowCount++; }
                contentHeight += Math.max(rowCount, 1) * 600;
            } else {
                contentHeight += 700;
            }
        }
        if (spacerIndices.length === 0) return;
        const budget = Math.round(availableTwips * 0.85) - contentHeight;
        if (budget >= spacerTotal) return;
        const scale = budget > 0 ? budget / spacerTotal : 0;
        for (const idx of spacerIndices) {
            const oldH = children[idx]._spacerHeight;
            const newH = Math.max(Math.round(oldH * scale), 20);
            children[idx] = spacer(newH);
        }
    }

    // ── Document assembly ─────────────────────────────────────────

    function createDoc(spec) {
        // LLMs frequently write `numbering: { config: [...] }` (docx-js's
        // shape) or a single `{ reference, levels }` object instead of the
        // array docx-helpers expects. Both should silently work.
        let _userNumbering = spec.numbering;
        if (_userNumbering && !Array.isArray(_userNumbering)) {
            if (Array.isArray(_userNumbering.config)) {
                _userNumbering = _userNumbering.config;
            } else if (_userNumbering.reference || _userNumbering.levels) {
                _userNumbering = [_userNumbering];
            } else {
                _userNumbering = [];
            }
        }
        const allNumbering = [..._builtinNumbering, ...(_userNumbering || [])];
        const registeredRefs = new Set(allNumbering.map(n => n.reference));
        for (const ref of _usedNumberingRefs) {
            if (!registeredRefs.has(ref)) {
                allNumbering.push({
                    reference: ref,
                    levels: _builtinNumbering.find(n => n.reference === NUMBER_REF).levels,
                });
            }
        }

        const _defaultPage = {
            size: {
                width: cfg.page.width,
                height: cfg.page.height,
                orientation: cfg.page.orientation || (
                    cfg.page.width > cfg.page.height
                        ? PageOrientation.LANDSCAPE
                        : PageOrientation.PORTRAIT
                ),
            },
            margin: cfg.page.margins,
        };

        if (!spec.sections || spec.sections.length === 0) {
            throw new Error('[docx-helpers] build() 的 sections 不能为空，至少需要一个 section');
        }
        const flatSections = [];
        const specPageNumbers = spec.pageNumbers ?? spec.page_numbers ?? spec.autoPageNumbers ?? spec.auto_page_numbers;
        for (const sec of spec.sections) {
            const flat = { ...sec };
            if (flat.children) {
                flat.children = Array.isArray(flat.children)
                    ? flat.children.flat(Infinity)
                    : [flat.children];
                flat.children = flat.children.filter(c => c != null).map(c => {
                    if (typeof c === 'string' || typeof c === 'number') {
                        return p(String(c));
                    }
                    return c;
                });
            }
            if (!flat.properties) {
                flat.properties = { page: _defaultPage, column: { count: 1 } };
            } else if (!flat.properties.page) {
                flat.properties.page = _defaultPage;
            }
            if (flat.cover && flat.children) {
                _shrinkCoverSpacers(flat.children, cfg.page.height - cfg.page.margins.top - cfg.page.margins.bottom);
            }
            const sectionPageNumbers = flat.pageNumbers ?? flat.page_numbers ?? flat.autoPageNumbers ?? flat.auto_page_numbers;
            const disablePageNumbers = specPageNumbers === false ||
                sectionPageNumbers === false ||
                flat.noPageNumber === true ||
                flat.noPageNumbers === true ||
                flat.no_page_number === true ||
                flat.no_page_numbers === true;
            delete flat.pageNumbers;
            delete flat.page_numbers;
            delete flat.autoPageNumbers;
            delete flat.auto_page_numbers;
            delete flat.noPageNumber;
            delete flat.noPageNumbers;
            delete flat.no_page_number;
            delete flat.no_page_numbers;
            if (!disablePageNumbers && !flat.footers) {
                flat.footers = { default: footer() };
            }
            flatSections.push(flat);
        }

        const lastSec = flatSections[flatSections.length - 1];
        const lastCol = lastSec && lastSec.properties && lastSec.properties.column;
        if (lastCol && lastCol.count > 1) {
            flatSections.push({
                properties: {
                    type: SectionType.CONTINUOUS,
                    page: _defaultPage,
                    column: { count: 1 },
                },
                children: [],
            });
        }

        if (_tocRef && _headingRegistry.length > 0) {
            const _tocTitleExcludes = new Set(['目录', 'tableofcontents']);
            const cachedEntries = _headingRegistry
                .filter(h => !_tocTitleExcludes.has(h.text.replace(/\s+/g, '').replace(/\u3000/g, '').toLowerCase()))
                .map(h => ({
                    title: h.text,
                    level: h.level - 1,
                    page: '',
                    ...(h.bookmark ? { href: h.bookmark } : {}),
                }));
            const newToc = new TableOfContents((_tocOpts && _tocOpts.title) || '\u76EE\u5F55', {
                hyperlink: !_tocOpts || _tocOpts.hyperlink !== false,
                headingStyleRange: (_tocOpts && _tocOpts.headingStyleRange) || '1-3',
                cachedEntries,
            });
            const newTocPara = p([newToc]);
            for (const sec of flatSections) {
                if (!sec.children) continue;
                const idx = sec.children.indexOf(_tocRef);
                if (idx !== -1) {
                    sec.children[idx] = newTocPara;
                    break;
                }
            }
        }

        const docSpec = {
            numbering: { config: allNumbering },
            sections: flatSections,
        };

        if (spec.compatibility) {
            docSpec.compatibility = spec.compatibility;
        }

        if (spec.styles) {
            docSpec.styles = spec.styles;
        } else {
            docSpec.styles = _defaultStyles();
        }

        if (spec.footnotes) docSpec.footnotes = spec.footnotes;

        return new Document(docSpec);
    }

    function _defaultStyles() {
        return {
            default: {
                document: {
                    run: { font: cfg.fonts.body, size: cfg.sizes.body, color: cfg.colors.text },
                    paragraph: { widowControl: true },
                },
            },
            paragraphStyles: [
                {
                    id: 'Heading1', name: 'Heading 1', basedOn: 'Normal', next: 'Normal', quickFormat: true,
                    run: { size: cfg.sizes.h1, bold: true, font: cfg.fonts.heading, color: cfg.colors.primary },
                    paragraph: { spacing: cfg.spacing.heading, outlineLevel: 0, keepNext: true, keepLines: true },
                },
                {
                    id: 'Heading2', name: 'Heading 2', basedOn: 'Normal', next: 'Normal', quickFormat: true,
                    run: { size: cfg.sizes.h2, bold: true, font: cfg.fonts.heading, color: cfg.colors.primary },
                    paragraph: { spacing: cfg.spacing.heading, outlineLevel: 1, keepNext: true, keepLines: true },
                },
                {
                    id: 'Heading3', name: 'Heading 3', basedOn: 'Normal', next: 'Normal', quickFormat: true,
                    run: { size: cfg.sizes.h3, bold: true, font: cfg.fonts.heading, color: cfg.colors.primary },
                    paragraph: { spacing: cfg.spacing.heading, outlineLevel: 2, keepNext: true, keepLines: true },
                },
            ],
        };
    }

    // 进程级一次性安全网：未 catch 的 Promise reject / 未捕获异常 强制 exitCode != 0，
    // 防止 build() 链路里抛错被吞导致 Python 端误判为成功。
    if (!global.__DOCX_HELPER_SAFETY_NET__) {
        global.__DOCX_HELPER_SAFETY_NET__ = true;
        process.on('unhandledRejection', (err) => {
            process.exitCode = 1;
            const msg = err && err.stack ? err.stack : String(err);
            console.error('[docx-helpers] unhandledRejection: ' + msg);
        });
        process.on('uncaughtException', (err) => {
            process.exitCode = 1;
            const msg = err && err.stack ? err.stack : String(err);
            console.error('[docx-helpers] uncaughtException: ' + msg);
        });
    }

    function _looksLikeSpec(v) {
        return v && typeof v === 'object' && !Array.isArray(v) && Array.isArray(v.sections);
    }
    function _looksLikePatch(v) {
        return v && typeof v === 'object' && !Array.isArray(v) && typeof v.type === 'string';
    }
    function _looksLikeSection(v) {
        return v && typeof v === 'object' && !Array.isArray(v) && Array.isArray(v.children);
    }
    function _looksLikePatchArray(a) {
        return Array.isArray(a) && a.length > 0 && a.every(_looksLikePatch);
    }
    function _looksLikeSectionArray(a) {
        return Array.isArray(a) && a.length > 0 && a.every(_looksLikeSection);
    }

    /**
     * 按"类型 + 形状"识别每个参数，把任意顺序/类型的输入归一化到 (spec, outputPath, patches)。
     * 兼容下列已知错位写法（均 warn 提示原代码可改进，但不阻断执行）:
     *   build(spec)                         ✓ 标准
     *   build(spec, [patches])              ✓ 标准
     *   build(spec, patchObj)               单 patch 对象 → 自动包成数组
     *   build(spec, '/path.docx')           显式给路径（不推荐但允许）
     *   build('/path', spec[, patches])     参数顺序写反
     *   build([patches], spec)              参数顺序写反
     *   build({sections, patches})          patches 错放进 spec
     *   build([section, section])           直接传 sections 数组而非 spec 对象
     *   build({children: [...]})            直接传单个 section 而非 spec
     *   build({spec, patches, output})      整体多包了一层
     */
    function _normalizeBuildArgs(rawArgs) {
        const warns = [];
        let spec = null;
        let outputPath = null;
        let patches = null;

        for (const a of rawArgs) {
            if (a == null) continue;

            if (typeof a === 'string') {
                if (outputPath == null) outputPath = a;
                continue;
            }

            if (Array.isArray(a)) {
                if (spec == null && _looksLikeSectionArray(a)) {
                    spec = { sections: a };
                    warns.push('build([section, ...]) → 已自动包成 { sections: [...] }');
                } else if (patches == null && _looksLikePatchArray(a)) {
                    patches = a;
                } else if (patches == null) {
                    patches = a; // 兜底：未知数组当 patches
                }
                continue;
            }

            if (typeof a === 'object') {
                if (_looksLikeSpec(a)) {
                    if (spec == null) spec = a;
                    continue;
                }
                if (a.spec && _looksLikeSpec(a.spec)) {
                    spec = a.spec;
                    if (typeof a.output === 'string' && outputPath == null) outputPath = a.output;
                    if (typeof a.outputPath === 'string' && outputPath == null) outputPath = a.outputPath;
                    if (Array.isArray(a.patches) && patches == null) patches = a.patches;
                    warns.push('build({spec, patches, output}) 多包了一层 → 已自动解构');
                    continue;
                }
                if (_looksLikeSection(a)) {
                    if (spec == null) {
                        spec = { sections: [a] };
                        warns.push('build({children: [...]}) 传的是单个 section → 已自动包成 { sections: [<section>] }');
                    }
                    continue;
                }
                if (_looksLikePatch(a)) {
                    if (patches == null) patches = [a];
                    else patches.push(a);
                    continue;
                }
            }
        }

        // patches 错放进 spec 顶层
        if (spec && Array.isArray(spec.patches) && patches == null) {
            patches = spec.patches;
            warns.push('build({sections, patches}) patches 错放进 spec → 已自动抽出');
        }

        return { spec, outputPath, patches, warns };
    }

    function build(...rawArgs) {
        // 兼容旧式 (spec, outputPath, patches) 显式三参数：先按位置识别参数顺序写反的常见 case 并 warn
        let order_warn = null;
        if (rawArgs.length >= 2 && typeof rawArgs[0] === 'string' && _looksLikeSpec(rawArgs[1])) {
            order_warn = 'h.build() 参数顺序写反 (build(outputPath, spec))';
        } else if (rawArgs.length >= 2 && Array.isArray(rawArgs[0]) && _looksLikeSpec(rawArgs[1])) {
            order_warn = 'h.build() 参数顺序写反 (build([patches], spec))';
        }

        const { spec, outputPath: _op, patches, warns } = _normalizeBuildArgs(rawArgs);
        if (order_warn) warns.unshift(order_warn);

        if (!spec) {
            throw new Error(
                '[docx-helpers] h.build() 无法识别出 spec 参数。\n' +
                '  → 唯一正确签名: h.build({ sections: [{ children: [...] }] }[, patches])\n' +
                '  → 不要传输出路径；路径由 Python 侧 run_node_docx(output=...) 自动注入。'
            );
        }
        if (!spec.sections.length) {
            throw new Error('[docx-helpers] h.build() spec.sections 不能为空数组');
        }

        let outputPath = _op || process.argv[2];
        if (!outputPath) {
            throw new Error(
                '[docx-helpers] 未指定输出路径：请通过 Python 侧 run_node_docx(output=...) 传入。'
            );
        }
        if (typeof outputPath !== 'string') {
            throw new Error('[docx-helpers] 输出路径必须是字符串，收到: ' + typeof outputPath);
        }

        // warns already collected; auto-compat applied silently

        if (_activeRefTracker && _activeRefTracker._citedKeys.size > 0 && !_activeRefTracker._bibGenerated) {
            const outDir = path.dirname(outputPath);
            const candidates = [
                path.join(outDir, 'references.json'),
                path.join(process.cwd(), 'references.json'),
            ];
            const jsonPath = candidates.find(p => fs.existsSync(p));
            if (jsonPath) {
                console.log(`[docx-helpers] 检测到正文引用但未调用 autoBibliography，自动追加参考文献 (${jsonPath})`);
                const bibChildren = _activeRefTracker.autoBibliography(jsonPath);
                if (bibChildren.length > 0) {
                    const lastSec = spec.sections[spec.sections.length - 1];
                    const refSection = { ...lastSec, children: [h1('参考文献'), ...bibChildren] };
                    spec.sections.push(refSection);
                }
            }
        }

        // started 标记：用于 Python 端兜底判断"build 至少进入了执行流程"
        console.log('[docx-helpers] build 开始执行 → ' + outputPath);

        const doc = createDoc(spec);
        const p = Packer.toBuffer(doc).then(async (buffer) => {
            let final = buffer;
            if (patches && patches.length > 0) {
                try {
                    const { applyPatches } = require(path.join(__dirname, 'docx_patches'));
                    final = await applyPatches(buffer, patches);
                } catch (err) {
                    throw new Error('[docx-helpers] applyPatches 失败: ' + err.message);
                }
            }
            const dir = path.dirname(outputPath);
            if (dir && !fs.existsSync(dir)) {
                fs.mkdirSync(dir, { recursive: true });
            }
            fs.writeFileSync(outputPath, final);
            // done 标记：必须出现，否则 Python 端会报错而不是 warn
            console.log('[docx-helpers] 文档已生成: ' + outputPath);
        });

        // 即便用户漏写 await/then，Node 也会等 Promise 完成；这里再加一层 catch 把错误显式转为非零退出
        p.catch((err) => {
            process.exitCode = 1;
            const msg = err && err.stack ? err.stack : String(err);
            console.error('[docx-helpers] build 失败: ' + msg);
        });

        return p;
    }

    // ── Math / Formula helpers ───────────────────────────────────

    function math(latex) {
        return _formula.createMath(latex);
    }

    function formula(latex, opts) {
        opts = opts || {};
        if (!opts._pageWidth) opts._pageWidth = contentWidth;
        if (!opts.spacing) {
            opts.spacing = {
                before: cfg.spacing.body.after != null ? Math.max(cfg.spacing.body.after, 120) : 160,
                after: cfg.spacing.body.after != null ? Math.max(cfg.spacing.body.after, 120) : 160,
                line: cfg.spacing.body.line,
                lineRule: cfg.spacing.body.lineRule,
            };
        }
        return _spreadableBlock(_formula.createFormula(latex, opts));
    }

    // ── Three-line table (学术三线表) ─────────────────────────────

    function threeLineTable(spec) {
        const {
            widths: _widths,
            width: _width,
            columnWidths: _columnWidths,
            header: hdr,
            rows = [],
            caption,
            captionPosition = 'top',
            cellFont,
            cellEastAsia,
            cellSize,
            margins,
        } = spec;

        let widths = _intArr(_widths || _columnWidths || (Array.isArray(_width) ? _width : null));
        if (!widths || !Array.isArray(widths)) {
            throw new Error('h.threeLineTable() 需要 widths 数组');
        }
        let totalWidth = widths.reduce((a, b) => a + b, 0);
        if (totalWidth > contentWidth && _columnCount > 1) {
            const testWidths = widths.map(w => Math.round(w * contentWidth / totalWidth));
            const minCol = Math.min(...testWidths);
            if (minCol < 700) {
                return fullWidth(() => threeLineTable(spec));
            }
        }
        if (totalWidth > contentWidth) {
            const scale = contentWidth / totalWidth;
            widths = widths.map(w => Math.round(w * scale));
            totalWidth = widths.reduce((a, b) => a + b, 0);
            const diff = contentWidth - totalWidth;
            if (diff !== 0) widths[widths.length - 1] += diff;
            totalWidth = contentWidth;
        }

        const THICK = { style: BorderStyle.SINGLE, size: 12, color: '000000' };
        const THIN = { style: BorderStyle.SINGLE, size: 6, color: '000000' };
        const NONE = { style: BorderStyle.NIL, size: 0 };
        const defaultMargins = _intObj(margins) || { top: 60, bottom: 60, left: 100, right: 100 };
        const numRows = (hdr ? 1 : 0) + rows.length;

        function makeBorders(rowIdx) {
            const isFirst = rowIdx === 0;
            const isHeaderBottom = hdr && rowIdx === 0;
            const isSecondRow = hdr && rowIdx === 1;
            const isLast = rowIdx === numRows - 1;
            return {
                top: isFirst ? THICK : isSecondRow ? THIN : NONE,
                bottom: isHeaderBottom ? THIN : isLast ? THICK : NONE,
                left: NONE, right: NONE,
            };
        }

        function makeCell(content, rowIdx, colIdx) {
            const cellProps = {
                width: { size: widths[colIdx], type: WidthType.DXA },
                borders: makeBorders(rowIdx),
                margins: defaultMargins,
            };
            const isHeader = hdr && rowIdx === 0;
            const cellContent = typeof content === 'string' ? content : String(content != null ? content : '');
            const runProps = { font: cellFont || cfg.fonts.body, size: _int(cellSize) || cfg.sizes.body };
            if (isHeader) runProps.bold = true;
            cellProps.children = [new Paragraph({
                alignment: AlignmentType.CENTER,
                spacing: { after: 0, line: 320 },
                children: _toChildren(cellContent, runProps),
            })];
            return new TableCell(cellProps);
        }

        function makeRow(cells, rowIdx) {
            return new TableRow({
                children: cells.map((c, ci) => makeCell(c, rowIdx, ci)),
            });
        }

        const allRows = [];
        let ri = 0;
        if (hdr) {
            if (hdr.length !== widths.length) {
                throw new Error(
                    `h.threeLineTable() header 有 ${hdr.length} 个单元格，但 widths 声明了 ${widths.length} 列`
                );
            }
            allRows.push(makeRow(hdr, ri)); ri++;
        }
        for (const row of rows) {
            if (row.length !== widths.length) {
                throw new Error(
                    `h.threeLineTable() 第 ${ri + 1} 行有 ${row.length} 个单元格，但 widths 声明了 ${widths.length} 列`
                );
            }
            allRows.push(makeRow(row, ri)); ri++;
        }

        const tbl = _spreadableBlock(new Table({
            width: { size: totalWidth, type: WidthType.DXA },
            columnWidths: widths,
            rows: allRows,
        }));

        const afterSpacer = new Paragraph({ spacing: { before: 200, after: 0, line: 20, lineRule: LineRuleType.EXACT } });

        if (!caption) return [tbl, afterSpacer];

        const captionPara = p(caption, {
            align: 'center',
            size: _int(cellSize) || 18,
            spacing: { before: captionPosition === 'top' ? 200 : 60, after: captionPosition === 'top' ? 60 : 200 },
        });
        return captionPosition === 'top' ? [captionPara, tbl, afterSpacer] : [tbl, captionPara];
    }

    // ── Reference tracker (引用追踪器) ──────────────────────────

    function refTracker(opts) {
        opts = opts || {};
        const _map = new Map();
        let _counter = 0;
        const citationStyle = String(opts.citationStyle || 'numeric').toLowerCase();
        const supSize = _int(opts.size) || _int(cfg.sizes.ref) || _int(cfg.sizes.small) || 18;
        const refFont = opts.font || cfg.fonts.english || cfg.fonts.body;
        const bodyFont = opts.bodyFont || cfg.fonts.body;
        const bodySize = _int(opts.bodySize) || _int(cfg.sizes.body) || 24;

        let _bibGenerated = false;

        function _getNum(key) {
            if (_map.has(key)) return _map.get(key);
            _counter++;
            _map.set(key, _counter);
            return _counter;
        }

        function _compressNums(nums) {
            if (nums.length <= 1) return nums.join('');
            const ranges = [];
            let start = nums[0], end = nums[0];
            for (let i = 1; i < nums.length; i++) {
                if (nums[i] === end + 1) { end = nums[i]; }
                else { ranges.push(start === end ? `${start}` : `${start}-${end}`); start = end = nums[i]; }
            }
            ranges.push(start === end ? `${start}` : `${start}-${end}`);
            return ranges.join(',');
        }

        function _authorYearFromKey(key) {
            const s = String(key || '').trim();
            if (!s) return '';
            const m = s.match(/^(.+?)(\d{4})([a-z])?$/i);
            if (!m) return s;
            const author = m[1];
            const year = m[2] + (m[3] ? m[3].toLowerCase() : '');
            return `${author}, ${year}`;
        }

        function _isAuthorYearStyle() {
            return citationStyle === 'apa' || citationStyle === 'author-year';
        }

        function _isIeeeStyle() {
            return citationStyle === 'ieee';
        }

        function _splitAuthors(raw) {
            const s = String(raw || '').trim().replace(/\.$/, '');
            if (!s) return [];
            return s.split(/\s*[;,，；]\s*/).map(a => a.trim()).filter(Boolean);
        }

        function _formatAuthorAPA(author) {
            const a = String(author || '').trim().replace(/^,|,$/g, '');
            if (!a) return '';
            if (/[\u4e00-\u9fff]/.test(a)) return a;
            const parts = a.split(/\s+/).filter(Boolean);
            if (parts.length === 1) return parts[0];
            const last = parts[parts.length - 1];
            const initials = parts.slice(0, -1).map(p => (p[0] ? `${p[0].toUpperCase()}.` : '')).filter(Boolean).join(' ');
            return `${last}, ${initials}`.trim().replace(/,\s*$/, '');
        }

        function _formatAuthorIEEE(author) {
            const a = String(author || '').trim().replace(/^,|,$/g, '');
            if (!a) return '';
            if (/[\u4e00-\u9fff]/.test(a)) return a;
            const parts = a.split(/\s+/).filter(Boolean);
            if (parts.length === 1) return parts[0];
            const last = parts[parts.length - 1];
            const initials = parts.slice(0, -1).map(p => (p[0] ? `${p[0].toUpperCase()}.` : '')).filter(Boolean).join(' ');
            return `${initials} ${last}`.trim();
        }

        function _formatAuthorsAPA(raw) {
            const authors = _splitAuthors(raw).map(_formatAuthorAPA).filter(Boolean);
            if (authors.length === 0) return '';
            if (authors.length === 1) return authors[0];
            if (authors.length === 2) return `${authors[0]}, & ${authors[1]}`;
            if (authors.length <= 20) return `${authors.slice(0, -1).join(', ')}, & ${authors[authors.length - 1]}`;
            return `${authors.slice(0, 19).join(', ')}, ... ${authors[authors.length - 1]}`;
        }

        function _formatAuthorsIEEE(raw) {
            const authors = _splitAuthors(raw).map(_formatAuthorIEEE).filter(Boolean);
            if (authors.length === 0) return '';
            if (authors.length <= 6) return authors.join(', ');
            return `${authors.slice(0, 6).join(', ')}, et al.`;
        }

        function _formatApaEntry(entry) {
            const authors = _formatAuthorsAPA(entry.authors || '');
            const year = String(entry.year || '').trim();
            const title = String(entry.title || '').trim().replace(/\.$/, '');
            const journal = String(entry.journal || '').trim().replace(/\.$/, '');
            const doi = String(entry.doi || '').trim();
            const url = String(entry.url || '').trim();
            const parts = [];
            if (authors) parts.push(authors);
            if (year) parts.push(`(${year}).`);
            if (title) parts.push(`${title}.`);
            if (journal) parts.push(`${journal}.`);
            if (doi) parts.push(doi.startsWith('http') ? doi : `https://doi.org/${doi}`);
            else if (url) parts.push(url);
            return parts.join(' ').replace(/\s+/g, ' ').trim();
        }

        function _formatIeeeEntry(entry) {
            const authors = _formatAuthorsIEEE(entry.authors || '');
            const title = String(entry.title || '').trim().replace(/\.$/, '');
            const journal = String(entry.journal || '').trim().replace(/\.$/, '');
            const year = String(entry.year || '').trim();
            const doi = String(entry.doi || '').trim();
            const url = String(entry.url || '').trim();
            const parts = [];
            if (authors) parts.push(`${authors},`);
            if (title) parts.push(`"${title},"`);
            if (journal) parts.push(`${journal},`);
            if (year) parts.push(`${year},`);
            if (doi) parts.push(`doi: ${doi}.`);
            else if (url) parts.push(`${url}.`);
            let text = parts.join(' ').replace(/\s+/g, ' ').trim();
            if (text.endsWith(',')) text = `${text.slice(0, -1)}.`;
            return text;
        }

        function _renderEntryText(entry, fallbackText) {
            if (citationStyle === 'apa') {
                const t = _formatApaEntry(entry);
                return t || fallbackText;
            }
            if (_isIeeeStyle()) {
                const t = _formatIeeeEntry(entry);
                return t || fallbackText;
            }
            return fallbackText;
        }

        function cite(...keys) {
            keys.forEach(k => _getNum(k));
            if (_isAuthorYearStyle()) {
                const labels = keys.map(k => _authorYearFromKey(k)).filter(Boolean);
                const text = labels.length > 0 ? `(${labels.join('; ')})` : '';
                return new TextRun({
                    text,
                    font: refFont,
                    size: bodySize,
                });
            }
            const nums = keys.map(k => _getNum(k));
            nums.sort((a, b) => a - b);
            const label = '[' + _compressNums(nums) + ']';
            return new TextRun({
                text: label,
                superScript: true,
                font: refFont,
                size: supSize,
            });
        }

        function bibliography(entries, bibOpts) {
            bibOpts = bibOpts || {};
            const _LEADING_MANUAL_REF_RE = /^(?:\s*\[(?:\d+(?:\s*[-,，]\s*\d+)*|[Nn])\]\s*)+/;

            const valid = entries.filter(e => e && typeof e === 'object' && e.key && e.text);

            const entryKeys = new Set(valid.map(e => e.key));
            const missing = [];
            for (const [key] of _map) {
                if (!entryKeys.has(key)) missing.push(key);
            }
            if (missing.length > 0 && !bibOpts._fromAutoBib) {
                console.warn(
                    `[docx-helpers] 警告: ${missing.length} 个正文 [@key] 在 bibliography 中缺少对应条目:\n` +
                    `  ${missing.join(', ')}\n` +
                    `  请确保每个 [@key] 都有对应的 { key, text } 条目。`
                );
            }

            const uncited = valid.filter(e => !_map.has(e.key));

            const sorted = [...valid].sort((a, b) => {
                if (_isAuthorYearStyle()) {
                    return String(a.text || '').localeCompare(String(b.text || ''), 'en');
                }
                const na = _map.has(a.key) ? _map.get(a.key) : Infinity;
                const nb = _map.has(b.key) ? _map.get(b.key) : Infinity;
                return na - nb;
            });
            const paragraphs = [];
            let strippedManualCount = 0;
            for (const entry of sorted) {
                const num = _map.has(entry.key) ? _map.get(entry.key) : (_counter++, _map.set(entry.key, _counter), _counter);
                const bodyRunProps = { font: bodyFont, size: bodySize };
                const rawText = typeof entry.text === 'string' ? entry.text : String(entry.text ?? '');
                const cleanText = rawText.replace(_LEADING_MANUAL_REF_RE, '').trimStart();
                const styledText = _renderEntryText(entry, cleanText);
                if (cleanText !== rawText) strippedManualCount++;
                const prefixRuns = _isAuthorYearStyle() ? [] : [
                    new TextRun({
                        text: `[${num}] `,
                        font: refFont,
                        size: bodySize,
                    }),
                ];
                paragraphs.push(new Paragraph({
                    spacing: { after: bibOpts.spacing || 60, line: bibOpts.line || 320 },
                    indent: { left: 420, hanging: 420 },
                    children: [
                        ...prefixRuns,
                        ..._toChildren(styledText, bodyRunProps),
                    ],
                }));
            }
            
            if (paragraphs.length > 0) _bibGenerated = true;
            return paragraphs;
        }

        function _parsePool(poolPath) {
            const raw = fs.readFileSync(poolPath, 'utf-8');
            const entries = [];
            let current = null;
            for (const line of raw.split('\n')) {
                if (line.startsWith('### ')) {
                    if (current) entries.push(current);
                    current = { key: line.slice(4).trim(), text: '', authors: '', title: '', year: '', journal: '', doi: '' };
                } else if (current) {
                    const m = line.match(/^- (.+?)：(.*)$/);
                    if (m) {
                        const [, field, val] = m;
                        const v = val.trim();
                        if (field === '引用文本') current.text = v;
                        else if (field === '作者') current.authors = v;
                        else if (field === '标题') current.title = v;
                        else if (field === '年份') current.year = v;
                        else if (field === '期刊/来源') current.journal = v;
                        else if (field === 'DOI') current.doi = v;
                    }
                }
            }
            if (current) entries.push(current);
            for (const e of entries) {
                if (!e.text) {
                    const parts = [e.authors, e.title].filter(Boolean).join('. ');
                    e.text = parts + (e.journal ? `[J]. ${e.journal}` : '') + (e.year ? `, ${e.year}` : '') + '.';
                }
            }
            return entries;
        }

        function bibliographyFromPool(poolPath, bibOpts) {
            const allEntries = _parsePool(poolPath);
            const cited = allEntries.filter(e => _map.has(e.key));
            if (cited.length === 0 && _map.size > 0) {
                console.warn(
                    `[docx-helpers] 文献池中未找到任何被引用的 key。\n` +
                    `  正文中引用了: ${[..._map.keys()].join(', ')}\n` +
                    `  文献池中有: ${allEntries.map(e => e.key).join(', ')}`
                );
            }
            return bibliography(cited.length > 0 ? cited : allEntries.filter(e => e.text), bibOpts);
        }

        function autoBibliography(jsonPath, bibOpts) {
            const raw = fs.readFileSync(jsonPath, 'utf-8');
            let entries;
            try {
                entries = JSON.parse(raw);
            } catch (e) {
                console.warn(`[docx-helpers] autoBibliography: JSON 解析失败: ${jsonPath}\n  ${e.message}`);
                return [];
            }
            if (entries && typeof entries === 'object' && !Array.isArray(entries)) {
                entries = Object.entries(entries).map(([k, v]) =>
                    (v && typeof v === 'object') ? { key: v.key || k, ...v } : { key: k }
                );
            }
            if (!Array.isArray(entries)) {
                console.warn(`[docx-helpers] autoBibliography: JSON 格式错误（期望数组或对象，实际为 ${typeof entries}）: ${jsonPath}`);
                return [];
            }
            if (entries.length === 0) {
                console.warn(`[docx-helpers] autoBibliography: 文献 JSON 为空，跳过参考文献生成: ${jsonPath}`);
                return [];
            }
            const valid = entries.filter(e => e && typeof e === 'object' && e.key && e.text);
            const cited = valid.filter(e => _map.has(e.key));

            const jsonKeys = new Set(valid.map(e => e.key));
            const missing = [..._map.keys()].filter(k => !jsonKeys.has(k));
            if (missing.length > 0) {
                console.warn(
                    `[docx-helpers] autoBibliography: ${missing.length} 个正文 [@key] 在 JSON 中无对应条目:\n` +
                    `  ${missing.join(', ')}\n` +
                    `  强制规则: 严禁手动补写 references.json 或手写参考文献条目；缺失引用直接忽略，不补检索、不补条目。`
                );
            }

            console.log(`[docx-helpers] 参考文献: 去除错误引用后实际引用 ${cited.length} 篇（以此为准，无需补充文献）`);

            return bibliography(cited, { ...bibOpts, _fromAutoBib: true });
        }

        const tracker = {
            cite, bibliography, bibliographyFromPool, autoBibliography,
            getNum: _getNum, get count() { return _counter; },
            get _bibGenerated() { return _bibGenerated; },
            get _citedKeys() { return _map; },
            _supSize: supSize, _refFont: refFont,
        };
        _activeRefTracker = tracker;
        return tracker;
    }

    // ── Public API ────────────────────────────────────────────────

    return {
        text, bold, italic,
        p, h1, h2, h3, bullet, numbered,
        table, tables: table, threeLineTable,
        math, formula,
        refTracker,
        pageBreak: pageBreak_, spacer, spizer: spacer, divider, fullWidth,
        img, link,
        header, footer, pageNum, headerFooter,
        coverBg, toc, bookmark, MERGE,
        createDoc, build,
        colors: cfg.colors,
        fonts: cfg.fonts,
        sizes: cfg.sizes,
        contentWidth,
        fullContentWidth: _fullContentWidth,
        cfg,
        raw: (() => {
            // LLMs frequently address docx-js constructors via dot
            // notation that doesn't exist in the upstream package,
            // e.g. `h.raw.Table.cell({...})` or `h.raw.Table.row({...})`.
            // Patching the constructors with non-enumerable aliases lets
            // those calls succeed without modifying user scripts. We use
            // try/catch because adding properties on the upstream classes
            // is best-effort (frozen objects would no-op silently).
            try {
                if (!Table.cell)     Object.defineProperty(Table, 'cell',     { value: TableCell, enumerable: false });
                if (!Table.row)      Object.defineProperty(Table, 'row',      { value: TableRow,  enumerable: false });
                if (!Paragraph.run)  Object.defineProperty(Paragraph, 'run',  { value: TextRun,   enumerable: false });
            } catch (_) {}
            return {
                Document, Packer, Paragraph, TextRun, Table, TableRow, TableCell,
                Header, Footer, HeadingLevel, BorderStyle, WidthType, ShadingType,
                AlignmentType, PageNumber, PageBreak, ImageRun, LevelFormat,
                ExternalHyperlink, InternalHyperlink, Bookmark, FootnoteReferenceRun,
                PageOrientation, TableOfContents, VerticalMergeType, VerticalAlign,
                TextDirection: TEXT_DIRECTION_RAW,
                PositionalTab, PositionalTabAlignment, PositionalTabRelativeTo, PositionalTabLeader,
                TabStopType, TabStopPosition, Column, SectionType, HeightRule: HEIGHT_RULE_RAW,
                HorizontalPositionRelativeFrom, VerticalPositionRelativeFrom,
            };
        })(),
    };
};
