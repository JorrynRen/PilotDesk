'use strict';

/**
 * LaTeX → docx Math objects converter.
 * Converts LaTeX math strings to docx-js Math elements (OMML output).
 *
 * Supported:
 *   Greek letters, operators, relations, fractions, sub/superscripts,
 *   roots, sum/integral/product, parentheses/brackets, accents,
 *   \text{}, \mathrm{}, \operatorname{}, \left...\right, matrices,
 *   \tag{} equation numbers.
 */

const {
    Math: OfficeMath, MathRun, MathFraction, MathSuperScript, MathSubScript,
    MathSubSuperScript, MathRadical, MathFunction, MathFunctionName,
    MathSum, MathIntegral, MathLimitLower, MathLimitUpper,
    MathRoundBrackets, MathSquareBrackets, MathCurlyBrackets, MathAngledBrackets,
    MathDegree, MathPreSubSuperScript,
    Paragraph, TextRun, AlignmentType, TabStopType, TabStopPosition,
    PositionalTab, PositionalTabAlignment, PositionalTabRelativeTo, PositionalTabLeader,
} = require('docx');

// ── Symbol maps ──────────────────────────────────────────────────

const GREEK = {
    alpha:'α', beta:'β', gamma:'γ', delta:'δ', epsilon:'ε', varepsilon:'ε',
    zeta:'ζ', eta:'η', theta:'θ', vartheta:'ϑ', iota:'ι', kappa:'κ',
    lambda:'λ', mu:'μ', nu:'ν', xi:'ξ', pi:'π', varpi:'ϖ',
    rho:'ρ', varrho:'ϱ', sigma:'σ', varsigma:'ς', tau:'τ', upsilon:'υ',
    phi:'φ', varphi:'φ', chi:'χ', psi:'ψ', omega:'ω',
    Gamma:'Γ', Delta:'Δ', Theta:'Θ', Lambda:'Λ', Xi:'Ξ', Pi:'Π',
    Sigma:'Σ', Upsilon:'Υ', Phi:'Φ', Psi:'Ψ', Omega:'Ω',
};

const OPERATORS = {
    times:'×', cdot:'·', cdots:'⋯', ldots:'…', ddots:'⋱', vdots:'⋮',
    div:'÷', pm:'±', mp:'∓', ast:'∗', star:'⋆', circ:'∘', bullet:'∙',
    oplus:'⊕', otimes:'⊗', odot:'⊙',
    cap:'∩', cup:'∪', vee:'∨', wedge:'∧', setminus:'∖',
    nabla:'∇', partial:'∂', infty:'∞', emptyset:'∅', varnothing:'∅',
    forall:'∀', exists:'∃', nexists:'∄', neg:'¬', lnot:'¬',
    in:'∈', notin:'∉', ni:'∋', subset:'⊂', supset:'⊃',
    subseteq:'⊆', supseteq:'⊇', subsetneq:'⊊', supsetneq:'⊋',
    approx:'≈', equiv:'≡', cong:'≅', sim:'∼', simeq:'≃', propto:'∝',
    neq:'≠', ne:'≠', leq:'≤', le:'≤', geq:'≥', ge:'≥',
    ll:'≪', gg:'≫', prec:'≺', succ:'≻',
    leftarrow:'←', rightarrow:'→', Leftarrow:'⇐', Rightarrow:'⇒',
    leftrightarrow:'↔', Leftrightarrow:'⇔', implies:'⇒', iff:'⇔',
    uparrow:'↑', downarrow:'↓', mapsto:'↦', longmapsto:'⟼',
    to:'→',
    prime:'′', angle:'∠', triangle:'△',
    hbar:'ℏ', ell:'ℓ', Re:'ℜ', Im:'ℑ', wp:'℘', aleph:'ℵ',
    quad:' ', qquad:'  ', ';':' ', ',':' ', '!':'', ':':' ',
    '%':'%', '#':'#', '&':'&', '{':'{', '}':'}', '_':'_',
    'backslash':'\\', 'mid':'∣', 'parallel':'∥',
    'perp':'⊥', 'models':'⊨', 'vdash':'⊢', 'dashv':'⊣',
    'top':'⊤', 'bot':'⊥',
    'langle':'⟨', 'rangle':'⟩', 'lceil':'⌈', 'rceil':'⌉',
    'lfloor':'⌊', 'rfloor':'⌋',
    'coloneqq':'\u2254', 'triangleq':'≜',
    'land':'∧', 'lor':'∨',
    'dots':'…', 'dotsb':'⋯', 'dotsc':'…', 'dotsm':'⋯', 'dotsi':'⋯',
    'lvert':'|', 'rvert':'|', 'vert':'|', 'lVert':'‖', 'rVert':'‖', 'Vert':'‖',
    'lbrace':'{', 'rbrace':'}',
};

const ACCENTS = {
    hat:'̂', widehat:'̂', bar:'̄', overline:'̄', tilde:'̃', widetilde:'̃',
    vec:'⃗', dot:'̇', ddot:'̈', acute:'́', grave:'̀', breve:'̆', check:'̌',
};

const NARY_MAP = {
    sum: '∑', prod: '∏', coprod: '∐',
    bigcup: '⋃', bigcap: '⋂', bigvee: '⋁', bigwedge: '⋀',
    bigoplus: '⨁', bigotimes: '⨂', bigodot: '⨀',
    biguplus: '⨄', bigsqcup: '⨆',
};

const INTEGRAL_MAP = {
    int: '∫', iint: '∬', iiint: '∭', oint: '∮',
};

const FUNC_NAMES = new Set([
    'sin','cos','tan','cot','sec','csc',
    'arcsin','arccos','arctan',
    'sinh','cosh','tanh','coth',
    'log','ln','exp','det','dim','ker','hom',
    'deg','gcd','lcm','max','min','sup','inf',
    'lim','limsup','liminf',
    'arg','Pr','mod',
    'softmax','Tr','diag','sign','sgn','rank',
]);

// ── Tokenizer ────────────────────────────────────────────────────

const TOK = {
    CMD: 'CMD', LBRACE: '{', RBRACE: '}', LBRACKET: '[', RBRACKET: ']',
    SUP: '^', SUB: '_', TEXT: 'TEXT', AMP: '&', DBLBACKSLASH: '\\\\',
    PIPE: '|', EOF: 'EOF',
};

function tokenize(latex) {
    const tokens = [];
    let i = 0;
    const s = latex;
    while (i < s.length) {
        const ch = s[i];
        if (ch === ' ' || ch === '\t') { i++; continue; }
        if (ch === '\n' || ch === '\r') { i++; continue; }
        if (ch === '{') { tokens.push({ type: TOK.LBRACE }); i++; continue; }
        if (ch === '}') { tokens.push({ type: TOK.RBRACE }); i++; continue; }
        if (ch === '[') { tokens.push({ type: TOK.LBRACKET }); i++; continue; }
        if (ch === ']') { tokens.push({ type: TOK.RBRACKET }); i++; continue; }
        if (ch === '^') { tokens.push({ type: TOK.SUP }); i++; continue; }
        if (ch === '_') { tokens.push({ type: TOK.SUB }); i++; continue; }
        if (ch === '&') { tokens.push({ type: TOK.AMP }); i++; continue; }
        if (ch === '|') { tokens.push({ type: TOK.PIPE }); i++; continue; }
        if (ch === '\\' && i + 1 < s.length && s[i + 1] === '\\') {
            tokens.push({ type: TOK.DBLBACKSLASH }); i += 2; continue;
        }
        if (ch === '\\') {
            i++;
            if (i < s.length && /[^a-zA-Z]/.test(s[i])) {
                tokens.push({ type: TOK.CMD, value: s[i] }); i++; continue;
            }
            let cmd = '';
            while (i < s.length && /[a-zA-Z]/.test(s[i])) { cmd += s[i]; i++; }
            tokens.push({ type: TOK.CMD, value: cmd });
            continue;
        }
        let text = '';
        while (i < s.length && !/[\\{}[\]^_&/+=(),\-\s]/.test(s[i])) { text += s[i]; i++; }
        if (text) {
            tokens.push({ type: TOK.TEXT, value: text });
        } else if (i < s.length) {
            tokens.push({ type: TOK.TEXT, value: s[i] }); i++;
        }
    }
    tokens.push({ type: TOK.EOF });
    return tokens;
}

// ── Parser ───────────────────────────────────────────────────────

class Parser {
    constructor(tokens) {
        this.tokens = tokens;
        this.pos = 0;
    }

    peek() { return this.tokens[this.pos]; }
    next() { return this.tokens[this.pos++]; }
    expect(type) {
        const t = this.next();
        if (t.type !== type) throw new Error(`Expected ${type}, got ${t.type}`);
        return t;
    }

    parse() {
        const nodes = this.parseSequence();
        return { type: 'seq', children: nodes };
    }

    parseSequence(stopTypes) {
        const nodes = [];
        while (true) {
            const t = this.peek();
            if (t.type === TOK.EOF) break;
            if (stopTypes && stopTypes.includes(t.type)) break;
            const node = this.parseAtom();
            if (!node) break;
            const withScripts = this.parseScripts(node);
            nodes.push(withScripts);
        }
        return nodes;
    }

    parseGroup() {
        this.expect(TOK.LBRACE);
        const children = this.parseSequence([TOK.RBRACE]);
        this.expect(TOK.RBRACE);
        if (children.length === 1) return children[0];
        return { type: 'seq', children };
    }

    parseOptionalBracket() {
        if (this.peek().type === TOK.LBRACKET) {
            this.next();
            let text = '';
            while (this.peek().type !== TOK.RBRACKET && this.peek().type !== TOK.EOF) {
                const t = this.next();
                text += t.value || t.type;
            }
            if (this.peek().type === TOK.RBRACKET) this.next();
            return text;
        }
        return null;
    }

    parseSingleArg() {
        const t = this.peek();
        if (t.type === TOK.LBRACE) return this.parseGroup();
        if (t.type === TOK.CMD) { this.next(); return this.buildCommandNode(t.value); }
        if (t.type === TOK.TEXT) {
            this.next();
            if (t.value.length > 1) {
                this.tokens.splice(this.pos, 0, { type: TOK.TEXT, value: t.value.slice(1) });
                return { type: 'text', value: t.value[0] };
            }
            return { type: 'text', value: t.value };
        }
        return { type: 'text', value: '' };
    }

    parseAtom() {
        const t = this.peek();
        if (t.type === TOK.LBRACE) return this.parseGroup();
        if (t.type === TOK.TEXT) { this.next(); return { type: 'text', value: t.value }; }
        if (t.type === TOK.AMP) { this.next(); return { type: 'align' }; }
        if (t.type === TOK.DBLBACKSLASH) { this.next(); return { type: 'newline' }; }
        if (t.type === TOK.PIPE) {
            this.next();
            return this.parsePipeGroup();
        }
        if (t.type === TOK.CMD) {
            this.next();
            return this.buildCommandNode(t.value);
        }
        return null;
    }

    parsePipeGroup() {
        const saved = this.pos;
        const children = [];
        let closeType = null;
        while (true) {
            const pk = this.peek();
            if (pk.type === TOK.EOF) break;
            if (pk.type === TOK.PIPE) { this.next(); closeType = '|'; break; }
            if (pk.type === TOK.CMD && (pk.value === 'rangle' || pk.value === 'rang')) {
                this.next();
                closeType = '\u27E9';
                break;
            }
            const node = this.parseAtom();
            if (!node) break;
            children.push(this.parseScripts(node));
        }
        if (closeType) {
            return { type: 'delimited', open: '|', close: closeType, children };
        }
        this.pos = saved;
        return { type: 'symbol', value: '|' };
    }

    buildCommandNode(cmd) {
        if (cmd === 'frac' || cmd === 'dfrac' || cmd === 'tfrac' || cmd === 'cfrac') {
            const num = this.parseSingleArg();
            const den = this.parseSingleArg();
            return { type: 'frac', num, den };
        }
        if (cmd === 'sqrt') {
            const deg = this.parseOptionalBracket();
            const body = this.parseSingleArg();
            return { type: 'sqrt', body, degree: deg };
        }
        if (cmd === 'text' || cmd === 'mathrm' || cmd === 'textrm' || cmd === 'textit'
            || cmd === 'mathit' || cmd === 'mathbf' || cmd === 'textbf'
            || cmd === 'mathbb' || cmd === 'mathcal' || cmd === 'mathsf'
            || cmd === 'operatorname') {
            const body = this.parseSingleArg();
            return { type: 'textcmd', cmd, body };
        }
        if (cmd === 'tag') {
            const body = this.parseSingleArg();
            return { type: 'tag', body };
        }
        if (cmd === 'left') {
            return this.parseLeftRight();
        }
        if (cmd === 'begin') {
            return this.parseEnvironment();
        }
        if (cmd === 'limits') {
            return { type: 'limits' };
        }
        if (cmd === 'not') {
            const inner = this.parseSingleArg();
            return { type: 'text', value: '≠' };
        }
        if (cmd === 'underbrace' || cmd === 'overbrace') {
            const body = this.parseSingleArg();
            return { type: 'accent_cmd', cmd, body };
        }
        if (ACCENTS[cmd]) {
            const body = this.parseSingleArg();
            return { type: 'accent', accent: cmd, body };
        }
        if (GREEK[cmd]) return { type: 'symbol', value: GREEK[cmd] };
        if (OPERATORS[cmd]) return { type: 'symbol', value: OPERATORS[cmd] };
        if (NARY_MAP[cmd]) return { type: 'nary', op: cmd, symbol: NARY_MAP[cmd] };
        if (INTEGRAL_MAP[cmd]) return { type: 'integral', op: cmd, symbol: INTEGRAL_MAP[cmd] };
        if (FUNC_NAMES.has(cmd)) {
            let arg = null;
            const pk = this.peek();
            if (pk.type === TOK.LBRACE) {
                arg = this.parseGroup();
            } else if (pk.type === TOK.CMD && ['frac','dfrac','tfrac','cfrac','sqrt','left'].includes(pk.value)) {
                this.next();
                arg = this.buildCommandNode(pk.value);
            }
            return { type: 'func', name: cmd, arg };
        }
        return { type: 'symbol', value: cmd };
    }

    _parseDelimiter() {
        const t = this.peek();
        if (t.type === TOK.TEXT) { this.next(); return t.value[0]; }
        if (t.type === TOK.CMD) { this.next(); return OPERATORS[t.value] || t.value; }
        if (t.type === TOK.LBRACE) { this.next(); return '{'; }
        if (t.type === TOK.RBRACE) { this.next(); return '}'; }
        if (t.type === TOK.LBRACKET) { this.next(); return '['; }
        if (t.type === TOK.RBRACKET) { this.next(); return ']'; }
        if (t.type === TOK.PIPE) { this.next(); return '|'; }
        return '';
    }

    parseLeftRight() {
        let open = this._parseDelimiter();
        if (open === '.') open = '';

        const children = [];
        while (true) {
            const pk = this.peek();
            if (pk.type === TOK.EOF) break;
            if (pk.type === TOK.CMD && pk.value === 'right') { this.next(); break; }
            if (pk.type === TOK.PIPE) {
                this.next();
                children.push(this.parseScripts({ type: 'symbol', value: '|' }));
                continue;
            }
            const node = this.parseAtom();
            if (!node) break;
            children.push(this.parseScripts(node));
        }

        let close = this._parseDelimiter();
        if (close === '.') close = '';

        return { type: 'delimited', open, close, children };
    }

    parseEnvironment() {
        this.expect(TOK.LBRACE);
        let envName = '';
        while (this.peek().type !== TOK.RBRACE && this.peek().type !== TOK.EOF) {
            envName += (this.next().value || '');
        }
        this.expect(TOK.RBRACE);

        if (['cases', 'matrix', 'pmatrix', 'bmatrix', 'vmatrix', 'Bmatrix', 'Vmatrix', 'aligned', 'array'].includes(envName)) {
            const rows = [];
            let currentRow = [];
            let currentCell = [];
            while (true) {
                const pk = this.peek();
                if (pk.type === TOK.EOF) break;
                if (pk.type === TOK.CMD && pk.value === 'end') {
                    this.next();
                    this.expect(TOK.LBRACE);
                    while (this.peek().type !== TOK.RBRACE && this.peek().type !== TOK.EOF) this.next();
                    if (this.peek().type === TOK.RBRACE) this.next();
                    break;
                }
                if (pk.type === TOK.AMP) {
                    this.next();
                    currentRow.push({ type: 'seq', children: currentCell });
                    currentCell = [];
                    continue;
                }
                if (pk.type === TOK.DBLBACKSLASH) {
                    this.next();
                    currentRow.push({ type: 'seq', children: currentCell });
                    rows.push(currentRow);
                    currentRow = [];
                    currentCell = [];
                    continue;
                }
                const node = this.parseAtom();
                if (node) currentCell.push(this.parseScripts(node));
            }
            if (currentCell.length > 0) currentRow.push({ type: 'seq', children: currentCell });
            if (currentRow.length > 0) rows.push(currentRow);
            return { type: 'matrix', envName, rows };
        }
        return { type: 'text', value: envName };
    }

    parseScripts(base) {
        let sub = null, sup = null;
        while (true) {
            const t = this.peek();
            if (t.type === TOK.SUB && !sub) {
                this.next();
                sub = this.parseSingleArg();
            } else if (t.type === TOK.SUP && !sup) {
                this.next();
                sup = this.parseSingleArg();
            } else {
                break;
            }
        }
        if (base && base.type === 'limits') return base;
        if (!sub && !sup) return base;
        if (sub && sup) return { type: 'subsup', base, sub, sup };
        if (sub) return { type: 'subscript', base, sub };
        return { type: 'superscript', base, sup };
    }
}

// ── Emitter (AST → docx Math objects) ────────────────────────────

function emit(node) {
    if (!node) return [_safeMathRun('')];
    if (Array.isArray(node)) return node.flatMap(emit);
    switch (node.type) {
        case 'text': return emitText(node.value);
        case 'symbol': return [_safeMathRun(node.value)];
        case 'seq': return node.children.flatMap(emit);
        case 'frac': return [emitFraction(node)];
        case 'sqrt': return [emitRadical(node)];
        case 'subscript': return [emitSubScript(node)];
        case 'superscript': return [emitSuperScript(node)];
        case 'subsup': return [emitSubSuperScript(node)];
        case 'nary': return [emitNary(node)];
        case 'integral': return [emitIntegral(node)];
        case 'func': return [emitFunc(node)];
        case 'delimited': return [emitDelimited(node)];
        case 'accent': return emitAccent(node);
        case 'accent_cmd': return emit(node.body);
        case 'textcmd': return emitTextCmd(node);
        case 'matrix': return [emitMatrix(node)];
        case 'tag': return [];
        case 'limits': return [];
        case 'align': return [_safeMathRun(' ')];
        case 'newline': return [_safeMathRun(' ')];
        default: return [_safeMathRun(node.value || '')];
    }
}

function emitText(value) {
    return [_safeMathRun(value)];
}

function emitFraction(node) {
    return new MathFraction({
        numerator: emit(node.num),
        denominator: emit(node.den),
    });
}

function emitRadical(node) {
    const opts = { children: emit(node.body) };
    if (node.degree) opts.degree = [_safeMathRun(node.degree)];
    return new MathRadical(opts);
}

function emitSubScript(node) {
    return new MathSubScript({
        children: emit(node.base),
        subScript: emit(node.sub),
    });
}

function emitSuperScript(node) {
    return new MathSuperScript({
        children: emit(node.base),
        superScript: emit(node.sup),
    });
}

function emitSubSuperScript(node) {
    const base = node.base;
    if (base && (base.type === 'nary' || base.type === 'integral')) {
        return emitNaryWithLimits(base, node.sub, node.sup);
    }
    return new MathSubSuperScript({
        children: emit(node.base),
        subScript: emit(node.sub),
        superScript: emit(node.sup),
    });
}

function emitNary(node) {
    return _safeMathRun(node.symbol);
}

function emitIntegral(node) {
    return _safeMathRun(node.symbol);
}

function emitNaryWithLimits(baseNode, sub, sup) {
    const symbol = _stripIllegalXmlChars(baseNode.symbol);
    const Cls = INTEGRAL_MAP[baseNode.op] ? MathIntegral : MathSum;
    return new Cls({
        children: [new MathRun(symbol)],
        subScript: emit(sub),
        superScript: emit(sup),
    });
}

function emitFunc(node) {
    if (node.arg) {
        return new MathFunction({
            name: [_safeMathRun(node.name)],
            children: emit(node.arg),
        });
    }
    return _safeMathRun(node.name);
}

function emitDelimited(node) {
    const innerChildren = node.children.flatMap(emit);
    const open = node.open || '(';
    const close = node.close || ')';

    const bracketMap = {
        '()': MathRoundBrackets,
        '[]': MathSquareBrackets,
        '{}': MathCurlyBrackets,
        '⟨⟩': MathAngledBrackets,
        '||': null,
        '‖‖': null,
    };
    const key = open + close;
    if (key === '||' || key === '‖‖' || !(key in bracketMap)) {
        return new MathRoundBrackets({ children: [
            _safeMathRun(open), ...innerChildren, _safeMathRun(close)
        ]});
    }
    return new (bracketMap[key])({ children: innerChildren });
}

function emitAccent(node) {
    const body = emit(node.body);
    const accentChar = ACCENTS[node.accent] || '';
    if (body.length === 1 && body[0] instanceof MathRun) {
        return [_safeMathRun(body[0].root?.[1]?.root?.[0]?.root?.[0] || '' + accentChar)];
    }
    return [...body, _safeMathRun(accentChar)];
}

function _isSimpleNode(node) {
    if (!node) return true;
    if (node.type === 'text' || node.type === 'symbol') return true;
    if (node.type === 'seq') return node.children.every(_isSimpleNode);
    return false;
}

function _toMathVariant(text, cmd) {
    if (cmd === 'mathbf' || cmd === 'textbf') {
        return [...text].map(ch => {
            const c = ch.charCodeAt(0);
            if (c >= 65 && c <= 90) return String.fromCodePoint(0x1D400 + c - 65);
            if (c >= 97 && c <= 122) return String.fromCodePoint(0x1D41A + c - 97);
            if (c >= 48 && c <= 57) return String.fromCodePoint(0x1D7CE + c - 48);
            return ch;
        }).join('');
    }
    if (cmd === 'mathbb') {
        const BB_EXCEPTIONS = { C:'\u2102', H:'\u210D', N:'\u2115', P:'\u2119', Q:'\u211A', R:'\u211D', Z:'\u2124' };
        return [...text].map(ch => {
            if (BB_EXCEPTIONS[ch]) return BB_EXCEPTIONS[ch];
            const c = ch.charCodeAt(0);
            if (c >= 65 && c <= 90) return String.fromCodePoint(0x1D538 + c - 65);
            if (c >= 97 && c <= 122) return String.fromCodePoint(0x1D552 + c - 97);
            if (c >= 48 && c <= 57) return String.fromCodePoint(0x1D7D8 + c - 48);
            return ch;
        }).join('');
    }
    if (cmd === 'mathcal') {
        const CAL_EXCEPTIONS = {
            B:'\u212C', E:'\u2130', F:'\u2131', H:'\u210B', I:'\u2110',
            L:'\u2112', M:'\u2133', R:'\u211B',
            e:'\u212F', g:'\u210A', o:'\u2134',
        };
        return [...text].map(ch => {
            if (CAL_EXCEPTIONS[ch]) return CAL_EXCEPTIONS[ch];
            const c = ch.charCodeAt(0);
            if (c >= 65 && c <= 90) return String.fromCodePoint(0x1D49C + c - 65);
            if (c >= 97 && c <= 122) return String.fromCodePoint(0x1D4B6 + c - 97);
            return ch;
        }).join('');
    }
    if (cmd === 'mathsf') {
        return [...text].map(ch => {
            const c = ch.charCodeAt(0);
            if (c >= 65 && c <= 90) return String.fromCodePoint(0x1D5A0 + c - 65);
            if (c >= 97 && c <= 122) return String.fromCodePoint(0x1D5BA + c - 97);
            if (c >= 48 && c <= 57) return String.fromCodePoint(0x1D7E2 + c - 48);
            return ch;
        }).join('');
    }
    return null;
}

function emitTextCmd(node) {
    const bodyText = extractText(node.body);

    if (_isSimpleNode(node.body)) {
        const variant = _toMathVariant(bodyText, node.cmd);
        if (variant) return [_safeMathRun(variant)];
        return [_safeMathRun(bodyText)];
    }
    return emit(node.body);
}

function extractText(node) {
    if (!node) return '';
    if (typeof node === 'string') return node;
    if (node.type === 'text') return node.value;
    if (node.type === 'symbol') return node.value;
    if (node.type === 'seq') return node.children.map(extractText).join('');
    return '';
}

function emitMatrix(node) {
    const envName = node.envName;
    const open = { pmatrix:'(', bmatrix:'[', vmatrix:'|', Bmatrix:'{', Vmatrix:'‖', cases:'{' }[envName] || '';
    const close = { pmatrix:')', bmatrix:']', vmatrix:'|', Bmatrix:'}', Vmatrix:'‖' }[envName] || '';

    const rowTexts = node.rows.map(row =>
        row.map(cell => emit(cell))
    );

    const flatChildren = [];
    for (let r = 0; r < rowTexts.length; r++) {
        for (let c = 0; c < rowTexts[r].length; c++) {
            if (c > 0) flatChildren.push(_safeMathRun('  '));
            flatChildren.push(...rowTexts[r][c]);
        }
        if (r < rowTexts.length - 1) flatChildren.push(_safeMathRun(' ; '));
    }

    if (open || close) {
        return new MathRoundBrackets({ children: flatChildren });
    }
    return new OfficeMath({ children: flatChildren });
}

// ── XML safety ───────────────────────────────────────────────────

/**
 * Strip characters that are illegal in XML 1.0 from a string.
 * XML 1.0 §2.2 allows: #x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] | [#x10000-#x10FFFF]
 * Everything else (notably U+0000–U+0008, U+000B–U+000C, U+000E–U+001F, U+FFFE–U+FFFF)
 * will cause XML parsers to reject the document.
 */
function _stripIllegalXmlChars(s) {
    if (!s) return s;
    return s.replace(/[\x00-\x08\x0B\x0C\x0E-\x1F\uFFFE\uFFFF]/g, '');
}

/**
 * Wrap MathRun to ensure its text content never contains illegal XML characters.
 * docx-js serializes MathRun text directly into <m:t>…</m:t> OMML elements;
 * control characters like U+0008 (backspace) slip through and corrupt the XML.
 */
function _safeMathRun(text) {
    return new MathRun(_stripIllegalXmlChars(text));
}

// ── Public API ───────────────────────────────────────────────────

/**
 * Parse a LaTeX string and return docx Math children array.
 * @param {string} latex - LaTeX math string (without $ delimiters)
 * @returns {Array} Array of docx Math child elements
 */
function latexToMathChildren(latex) {
    if (!latex || !latex.trim()) return [_safeMathRun('')];
    try {
        const tokens = tokenize(latex.trim());
        const parser = new Parser(tokens);
        const ast = parser.parse();
        return emit(ast);
    } catch (e) {
        console.warn('[formula] Parse error, falling back to plain text:', e.message);
        return [_safeMathRun(latex)];
    }
}

/**
 * Extract \tag{...} from LaTeX string.
 * @returns {{ latex: string, tag: string }}
 */
function extractTag(latex) {
    const m = latex.match(/\\tag\{([^}]*)\}/);
    if (m) {
        return {
            latex: latex.replace(/\s*\\tag\{[^}]*\}/, '').trim(),
            tag: m[1].trim(),
        };
    }
    return { latex, tag: '' };
}

/**
 * Create an inline OfficeMath element from LaTeX.
 * @param {string} latex
 * @returns {OfficeMath}
 */
function createMath(latex) {
    const children = latexToMathChildren(latex);
    return new OfficeMath({ children });
}

/**
 * Create a centered block formula paragraph with optional equation number.
 * @param {string} latex - LaTeX math string
 * @param {object} [opts] - Options
 * @param {string} [opts.eqNumber] - Equation number (e.g. '3-1')
 * @param {string} [opts.font] - Font for equation number
 * @param {number} [opts.size] - Font size (half-points) for equation number
 * @returns {Paragraph[]} Array of paragraphs (usually 1)
 */
function createFormula(latex, opts) {
    opts = opts || {};
    const { latex: cleanLatex, tag } = extractTag(latex);
    const eqNum = opts.eqNumber || tag;
    const pageWidth = opts._pageWidth || 9072;

    const mathChildren = latexToMathChildren(cleanLatex);

    const spacing = opts.spacing || { before: 160, after: 160 };

    if (!eqNum) {
        return new Paragraph({
            alignment: AlignmentType.CENTER,
            spacing,
            children: [new OfficeMath({ children: mathChildren })],
        });
    }

    const centerPos = Math.round(pageWidth / 2);
    return new Paragraph({
        spacing,
        tabStops: [
            { type: TabStopType.CENTER, position: centerPos },
            { type: TabStopType.RIGHT, position: pageWidth },
        ],
        children: [
            new TextRun({ children: [new PositionalTab({
                alignment: PositionalTabAlignment.CENTER,
                relativeTo: PositionalTabRelativeTo.MARGIN,
                leader: PositionalTabLeader.NONE,
            })] }),
            new OfficeMath({ children: mathChildren }),
            new TextRun({ children: [new PositionalTab({
                alignment: PositionalTabAlignment.RIGHT,
                relativeTo: PositionalTabRelativeTo.MARGIN,
                leader: PositionalTabLeader.NONE,
            })] }),
            new TextRun({
                text: `(${eqNum})`,
                font: opts.font || 'Times New Roman',
                size: opts.size || 24,
            }),
        ],
    });
}

module.exports = {
    latexToMathChildren,
    createMath,
    createFormula,
    extractTag,
};
