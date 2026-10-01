/**
 * DOCX 后处理补丁（JS 版）—— 补充 docx-js 无法覆盖的排版能力。
 *
 * 两种用法：
 *
 * 【推荐】在生成脚本内直接使用（零额外 I/O）:
 *   const { applyPatches } = require('./docx_patches');
 *   const buffer = await Packer.toBuffer(doc);
 *   const patched = await applyPatches(buffer, [
 *     { type: 'watermark', text: 'DRAFT' },
 *     { type: 'stripe',    evenFill: 'F2F7FC', headerFill: '2F5897' },
 *     { type: 'dropCap',   paragraphIndex: 0, lines: 3 },
 *     { type: 'backgroundImage', imagePath: 'bg.png' },
 *     { type: 'coverColor', colors: ['1A1A2E', '3D1F0B'], direction: 'vertical' },
 *     { type: 'pieChart', title: '份额', categories: ['A','B','C'], values: [40,35,25], placeholderId: 'market-share' },
 *     { type: 'barChart', series: [{name:'收入', categories:['Q1','Q2'], values:[100,200]}], placeholderId: 'revenue' },
 *     { type: 'lineChart', series: [{name:'趋势', categories:['1月','2月'], values:[10,20]}], placeholderId: 'trend' },
 *   ]);
 *   fs.writeFileSync('xxx.docx', patched);
 *
 * 【独立运行】对已有 docx 文件补丁:
 *   node docx_patches.js xxx.docx watermark "机密" --color=FF0000
 *   node docx_patches.js xxx.docx stripe --evenFill=E8F0FE
 */

const JSZip = require('jszip');
const { DOMParser, XMLSerializer } = require('@xmldom/xmldom');
const fs = require('fs');
const path = require('path');

// ── XML helpers ───────────────────────────────────────────────────

const W = 'http://schemas.openxmlformats.org/wordprocessingml/2006/main';
const R = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships';
const REL = 'http://schemas.openxmlformats.org/package/2006/relationships';
const CT = 'http://schemas.openxmlformats.org/package/2006/content-types';
const WP = 'http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing';
const A = 'http://schemas.openxmlformats.org/drawingml/2006/main';
const PIC = 'http://schemas.openxmlformats.org/drawingml/2006/picture';
const C = 'http://schemas.openxmlformats.org/drawingml/2006/chart';
const WPS = 'http://schemas.microsoft.com/office/word/2010/wordprocessingShape';
const V = 'urn:schemas-microsoft-com:vml';
const O = 'urn:schemas-microsoft-com:office:office';

const HEADER_CT = 'application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml';
const CHART_CT = 'application/vnd.openxmlformats-officedocument.drawingml.chart+xml';
const HEADER_REL = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships/header';
const CHART_REL = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart';
const IMAGE_REL = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships/image';

const CHART_COLORS = [
    '4472C4', 'ED7D31', 'A5A5A5', 'FFC000', '5B9BD5',
    '70AD47', '264478', '9B57A0', '636363', 'EB7D3C',
];

function parseXml(str) {
    return new DOMParser().parseFromString(str, 'text/xml');
}

function serXml(doc) {
    const raw = new XMLSerializer().serializeToString(doc);
    const stripped = raw.replace(/<\?xml[^?]*\?>\s*/g, '');
    return '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>' + stripped;
}

function getOrCreate(zip, filePath, defaultXml) {
    const existing = zip.file(filePath);
    if (existing) return existing.async('string').then(parseXml);
    return Promise.resolve(parseXml(defaultXml));
}

function maxRid(relsDoc) {
    let max = 0;
    const rels = relsDoc.getElementsByTagNameNS(REL, 'Relationship');
    for (let i = 0; i < rels.length; i++) {
        const id = rels[i].getAttribute('Id') || '';
        if (id.startsWith('rId')) {
            const n = parseInt(id.slice(3), 10);
            if (n > max) max = n;
        }
    }
    return max;
}

function findRelByType(relsDoc, type) {
    const rels = relsDoc.getElementsByTagNameNS(REL, 'Relationship');
    for (let i = 0; i < rels.length; i++) {
        if (rels[i].getAttribute('Type') === type) {
            return { target: rels[i].getAttribute('Target'), id: rels[i].getAttribute('Id') };
        }
    }
    return null;
}

function addRel(relsDoc, type, target) {
    const newId = `rId${maxRid(relsDoc) + 1}`;
    const rel = relsDoc.createElementNS(REL, 'Relationship');
    rel.setAttribute('Id', newId);
    rel.setAttribute('Type', type);
    rel.setAttribute('Target', target);
    relsDoc.documentElement.appendChild(rel);
    return newId;
}

const EMPTY_RELS = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="${REL}"></Relationships>`;

const EMPTY_HEADER = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:hdr xmlns:w="${W}" xmlns:r="${R}"><w:p><w:pPr/></w:p></w:hdr>`;

async function ensureHeader(zip) {
    const docRelsPath = 'word/_rels/document.xml.rels';
    const relsDoc = await getOrCreate(zip, docRelsPath, EMPTY_RELS);

    const existing = findRelByType(relsDoc, HEADER_REL);
    if (existing) {
        return { headerPath: `word/${existing.target}`, rId: existing.id, relsDoc, docRelsPath };
    }

    const headerName = 'header1.xml';
    zip.file(`word/${headerName}`, EMPTY_HEADER);

    const ctDoc = await zip.file('[Content_Types].xml').async('string').then(parseXml);
    const override = ctDoc.createElementNS(CT, 'Override');
    override.setAttribute('PartName', `/word/${headerName}`);
    override.setAttribute('ContentType', HEADER_CT);
    ctDoc.documentElement.appendChild(override);
    zip.file('[Content_Types].xml', serXml(ctDoc));

    const rId = addRel(relsDoc, HEADER_REL, headerName);
    zip.file(docRelsPath, serXml(relsDoc));

    const docDoc = await zip.file('word/document.xml').async('string').then(parseXml);
    const sects = docDoc.getElementsByTagNameNS(W, 'sectPr');
    for (let i = 0; i < sects.length; i++) {
        const ref = docDoc.createElementNS(W, 'w:headerReference');
        ref.setAttributeNS(W, 'w:type', 'default');
        ref.setAttributeNS(R, 'r:id', rId);
        sects[i].appendChild(ref);
    }
    zip.file('word/document.xml', serXml(docDoc));

    return { headerPath: `word/${headerName}`, rId, relsDoc, docRelsPath };
}


// ══════════════════════════════════════════════════════════════════
// Patch 1: Text watermark
// ══════════════════════════════════════════════════════════════════

const SHAPETYPE_XML = `<v:shapetype xmlns:v="${V}" xmlns:o="${O}" id="_x0000_t136" coordsize="21600,21600" o:spt="136" adj="10800" path="m@7,l@8,m@5,21600l@6,21600e"><v:formulas><v:f eqn="sum #0 0 10800"/><v:f eqn="prod #0 2 1"/><v:f eqn="sum 21600 0 @1"/><v:f eqn="sum 0 0 @2"/><v:f eqn="sum 21600 0 @3"/><v:f eqn="if @0 @3 0"/><v:f eqn="if @0 21600 @1"/><v:f eqn="if @0 0 @2"/><v:f eqn="if @0 @4 21600"/><v:f eqn="mid @5 @6"/><v:f eqn="mid @8 @5"/><v:f eqn="mid @7 @8"/><v:f eqn="mid @6 @7"/><v:f eqn="sum @6 0 @5"/></v:formulas><v:path textpathok="t" o:connecttype="custom" o:connectlocs="@9,0;@10,10800;@11,21600;@12,10800" o:connectangles="270,180,90,0"/><v:textpath on="t" fitshape="t"/><v:handles><v:h position="#0,bottomRight" xrange="6629,14971"/></v:handles><o:lock v:ext="edit" text="t" shapetype="t"/></v:shapetype>`;

async function patchWatermark(zip, opts = {}) {
    const {
        text = 'DRAFT',
        color = 'C0C0C0',
        font = 'Calibri',
        opacity = 0.5,
        rotation = -45,
    } = opts;

    const { headerPath } = await ensureHeader(zip);
    const hdrDoc = await zip.file(headerPath).async('string').then(parseXml);

    const rot = rotation < 0 ? 360 + rotation : rotation;
    const style = `position:absolute;margin-left:0;margin-top:0;width:494.95pt;height:164.95pt;rotation:${rot};z-index:-251658752;mso-position-horizontal:center;mso-position-horizontal-relative:margin;mso-position-vertical:center;mso-position-vertical-relative:margin`;

    const wmXml = `<w:p xmlns:w="${W}" xmlns:v="${V}" xmlns:o="${O}"><w:pPr><w:pStyle w:val="Header"/></w:pPr><w:r><w:rPr><w:noProof/></w:rPr><w:pict>${SHAPETYPE_XML}<v:shape id="WaterMark" o:spid="_x0000_s2049" type="#_x0000_t136" style="${style}" o:allowincell="f" fillcolor="#${color}" stroked="f"><v:fill opacity="${opacity}"/><v:textpath style='font-family:"${font}";font-size:1pt' string="${text}"/></v:shape></w:pict></w:r></w:p>`;

    const wmDoc = parseXml(wmXml);
    const wmP = wmDoc.documentElement;
    const imported = hdrDoc.importNode(wmP, true);
    hdrDoc.documentElement.insertBefore(imported, hdrDoc.documentElement.firstChild);

    zip.file(headerPath, serXml(hdrDoc));
}


// ══════════════════════════════════════════════════════════════════
// Patch 2: Drop cap
// ══════════════════════════════════════════════════════════════════

async function patchDropCap(zip, opts = {}) {
    const {
        paragraphIndex = 0,
        placeholderId,
        lines = 3,
        style = 'drop',
    } = opts;

    const docDoc = await zip.file('word/document.xml').async('string').then(parseXml);
    const body = docDoc.getElementsByTagNameNS(W, 'body')[0];
    if (!body) return;

    const paragraphs = [];
    for (let child = body.firstChild; child; child = child.nextSibling) {
        if (child.nodeType === 1 && child.localName === 'p') paragraphs.push(child);
    }

    let targetP;
    if (placeholderId) {
        const marker = `{{DROPCAP:${placeholderId}}}`;
        targetP = paragraphs.find(p => (p.textContent || '').includes(marker));
        if (!targetP) return;
    } else {
        if (paragraphIndex >= paragraphs.length) return;
        targetP = paragraphs[paragraphIndex];
    }
    const runs = targetP.getElementsByTagNameNS(W, 'r');
    if (runs.length === 0) return;

    const firstRun = runs[0];
    const tNodes = firstRun.getElementsByTagNameNS(W, 't');
    if (tNodes.length === 0 || !tNodes[0].textContent) return;

    const firstChar = tNodes[0].textContent[0];
    const rest = tNodes[0].textContent.slice(1);

    const capP = docDoc.createElementNS(W, 'w:p');
    const capPPr = docDoc.createElementNS(W, 'w:pPr');
    const framePr = docDoc.createElementNS(W, 'w:framePr');
    framePr.setAttributeNS(W, 'w:dropCap', style);
    framePr.setAttributeNS(W, 'w:lines', String(lines));
    framePr.setAttributeNS(W, 'w:wrap', 'around');
    framePr.setAttributeNS(W, 'w:vAnchor', 'text');
    framePr.setAttributeNS(W, 'w:hAnchor', 'text');
    capPPr.appendChild(framePr);
    capP.appendChild(capPPr);

    const capRun = docDoc.createElementNS(W, 'w:r');
    const existingRPr = firstRun.getElementsByTagNameNS(W, 'rPr')[0];
    if (existingRPr) capRun.appendChild(existingRPr.cloneNode(true));
    const capT = docDoc.createElementNS(W, 'w:t');
    capT.textContent = firstChar;
    capRun.appendChild(capT);
    capP.appendChild(capRun);

    tNodes[0].textContent = rest;

    body.insertBefore(capP, targetP);

    zip.file('word/document.xml', serXml(docDoc));
}


// ══════════════════════════════════════════════════════════════════
// Patch 3: Table auto-stripe
// ══════════════════════════════════════════════════════════════════

async function patchStripeTables(zip, opts = {}) {
    const {
        evenFill = 'F2F2F2',
        headerFill = null,
    } = opts;

    const docDoc = await zip.file('word/document.xml').async('string').then(parseXml);
    const tables = docDoc.getElementsByTagNameNS(W, 'tbl');

    for (let t = 0; t < tables.length; t++) {
        const rows = tables[t].getElementsByTagNameNS(W, 'tr');
        for (let ri = 0; ri < rows.length; ri++) {
            let fill = null;
            if (ri === 0 && headerFill) fill = headerFill;
            else if (ri > 0 && ri % 2 === 0) fill = evenFill;

            if (!fill) continue;

            const cells = rows[ri].getElementsByTagNameNS(W, 'tc');
            for (let ci = 0; ci < cells.length; ci++) {
                let tcPr = cells[ci].getElementsByTagNameNS(W, 'tcPr')[0];
                if (!tcPr) {
                    tcPr = docDoc.createElementNS(W, 'w:tcPr');
                    cells[ci].insertBefore(tcPr, cells[ci].firstChild);
                }
                if (tcPr.getElementsByTagNameNS(W, 'shd').length > 0) continue;

                const shd = docDoc.createElementNS(W, 'w:shd');
                shd.setAttributeNS(W, 'w:val', 'clear');
                shd.setAttributeNS(W, 'w:color', 'auto');
                shd.setAttributeNS(W, 'w:fill', fill);
                tcPr.appendChild(shd);
            }
        }
    }

    zip.file('word/document.xml', serXml(docDoc));
    return tables.length;
}


// ══════════════════════════════════════════════════════════════════
// Patch 4: Full-page background image
// ══════════════════════════════════════════════════════════════════

async function readPageSizeEmu(zip) {
    const docXml = await zip.file('word/document.xml').async('string');
    const docDoc = parseXml(docXml);
    const sects = docDoc.getElementsByTagNameNS(W, 'sectPr');
    if (sects.length === 0) return null;
    const last = sects[sects.length - 1];
    const pgSz = last.getElementsByTagNameNS(W, 'pgSz');
    if (pgSz.length === 0) return null;
    const wAttr = pgSz[0].getAttributeNS(W, 'w') || pgSz[0].getAttribute('w:w');
    const hAttr = pgSz[0].getAttributeNS(W, 'h') || pgSz[0].getAttribute('w:h');
    if (!wAttr || !hAttr) return null;
    const DXA_TO_EMU = 914400 / 1440;
    return {
        width: Math.round(parseInt(wAttr, 10) * DXA_TO_EMU),
        height: Math.round(parseInt(hAttr, 10) * DXA_TO_EMU),
    };
}

async function patchBackgroundImage(zip, opts = {}) {
    const {
        imagePath,
        pageWidthEmu,
        pageHeightEmu,
    } = opts;

    if (!imagePath) throw new Error('imagePath is required');

    const autoSize = await readPageSizeEmu(zip);
    const cx = String(pageWidthEmu || (autoSize && autoSize.width) || 7772400);
    const cy = String(pageHeightEmu || (autoSize && autoSize.height) || 10058400);
    const imageData = fs.readFileSync(imagePath);
    const ext = path.extname(imagePath).slice(1).toLowerCase();
    const mediaName = `bg_${Date.now()}.${ext}`;

    zip.file(`word/media/${mediaName}`, imageData);

    const mime = { png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif' }[ext] || 'image/png';
    const ctDoc = await zip.file('[Content_Types].xml').async('string').then(parseXml);
    const defaults = ctDoc.getElementsByTagNameNS(CT, 'Default');
    let hasExt = false;
    for (let i = 0; i < defaults.length; i++) {
        if (defaults[i].getAttribute('Extension') === ext) { hasExt = true; break; }
    }
    if (!hasExt) {
        const def = ctDoc.createElementNS(CT, 'Default');
        def.setAttribute('Extension', ext);
        def.setAttribute('ContentType', mime);
        ctDoc.documentElement.appendChild(def);
        zip.file('[Content_Types].xml', serXml(ctDoc));
    }

    const { headerPath } = await ensureHeader(zip);
    const headerName = path.basename(headerPath);
    const hdrRelsPath = `word/_rels/${headerName}.rels`;
    const hdrRelsDoc = await getOrCreate(zip, hdrRelsPath, EMPTY_RELS);
    const imgRid = addRel(hdrRelsDoc, IMAGE_REL, `media/${mediaName}`);
    zip.file(hdrRelsPath, serXml(hdrRelsDoc));

    const hdrDoc = await zip.file(headerPath).async('string').then(parseXml);

    const drawingXml = `<w:p xmlns:w="${W}" xmlns:r="${R}" xmlns:wp="${WP}" xmlns:a="${A}" xmlns:pic="${PIC}"><w:r><w:rPr><w:noProof/></w:rPr><w:drawing><wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" relativeHeight="0" behindDoc="1" locked="1" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="page"><wp:posOffset>0</wp:posOffset></wp:positionH><wp:positionV relativeFrom="page"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx="${cx}" cy="${cy}"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapNone/><wp:docPr id="100" name="Background"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="0" name="bg"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="${imgRid}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="${cx}" cy="${cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:anchor></w:drawing></w:r></w:p>`;

    const bgDoc = parseXml(drawingXml);
    const bgP = hdrDoc.importNode(bgDoc.documentElement, true);
    hdrDoc.documentElement.insertBefore(bgP, hdrDoc.documentElement.firstChild);

    zip.file(headerPath, serXml(hdrDoc));
}


// ══════════════════════════════════════════════════════════════════
// Patch 5: Cover color (solid / gradient) via DrawingML shape
// ══════════════════════════════════════════════════════════════════

function _buildFillXml(colors) {
    if (colors.length === 1) {
        return `<a:solidFill xmlns:a="${A}"><a:srgbClr val="${colors[0]}"/></a:solidFill>`;
    }
    let gsItems = '';
    for (let i = 0; i < colors.length; i++) {
        const pos = Math.round((i / (colors.length - 1)) * 100000);
        gsItems += `<a:gs pos="${pos}"><a:srgbClr val="${colors[i]}"/></a:gs>`;
    }
    return `<a:gradFill xmlns:a="${A}"><a:gsLst>${gsItems}</a:gsLst>` +
        `<a:lin ang="__ANG__" scaled="1"/></a:gradFill>`;
}

async function patchCoverColor(zip, opts = {}) {
    const { colors, direction = 'vertical' } = opts;
    if (!colors || colors.length === 0) throw new Error('coverColor: colors is required');

    const ang = direction === 'horizontal' ? '0' : '5400000';
    const autoSize = await readPageSizeEmu(zip);
    const cx = String((autoSize && autoSize.width) || 7772400);
    const cy = String((autoSize && autoSize.height) || 10058400);

    const fillXml = _buildFillXml(colors).replace('__ANG__', ang);

    const shapeXml =
        `<w:p xmlns:w="${W}" xmlns:r="${R}" xmlns:wp="${WP}" xmlns:a="${A}" xmlns:wps="${WPS}">` +
        `<w:r><w:drawing>` +
        `<wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" ` +
        `relativeHeight="0" behindDoc="1" locked="1" layoutInCell="1" allowOverlap="1">` +
        `<wp:simplePos x="0" y="0"/>` +
        `<wp:positionH relativeFrom="page"><wp:posOffset>0</wp:posOffset></wp:positionH>` +
        `<wp:positionV relativeFrom="page"><wp:posOffset>0</wp:posOffset></wp:positionV>` +
        `<wp:extent cx="${cx}" cy="${cy}"/>` +
        `<wp:effectExtent l="0" t="0" r="0" b="0"/>` +
        `<wp:wrapNone/>` +
        `<wp:docPr id="998" name="CoverColorBg"/>` +
        `<a:graphic><a:graphicData uri="${WPS}">` +
        `<wps:wsp>` +
        `<wps:spPr>` +
        `<a:xfrm><a:off x="0" y="0"/><a:ext cx="${cx}" cy="${cy}"/></a:xfrm>` +
        `<a:prstGeom prst="rect"><a:avLst/></a:prstGeom>` +
        fillXml +
        `<a:ln><a:noFill/></a:ln>` +
        `</wps:spPr>` +
        `<wps:bodyPr/>` +
        `</wps:wsp>` +
        `</a:graphicData></a:graphic>` +
        `</wp:anchor>` +
        `</w:drawing></w:r></w:p>`;

    const docDoc = await zip.file('word/document.xml').async('string').then(parseXml);
    const body = docDoc.getElementsByTagNameNS(W, 'body')[0];
    if (!body) return;

    const shapeDoc = parseXml(shapeXml);
    const imported = docDoc.importNode(shapeDoc.documentElement, true);
    body.insertBefore(imported, body.firstChild);

    zip.file('word/document.xml', serXml(docDoc));
}


// ══════════════════════════════════════════════════════════════════
// Patch 6: Native OOXML charts (pie / bar / line)
// ══════════════════════════════════════════════════════════════════

let _chartCounter = 0;

function _nextChartId() {
    _chartCounter++;
    return _chartCounter;
}

function _strLitXml(categories) {
    const arr = Array.isArray(categories) ? categories : [];
    let xml = `<c:strLit xmlns:c="${C}"><c:ptCount val="${arr.length}"/>`;
    for (let i = 0; i < arr.length; i++) {
        xml += `<c:pt idx="${i}"><c:v>${_escXml(String(arr[i] != null ? arr[i] : ''))}</c:v></c:pt>`;
    }
    xml += '</c:strLit>';
    return xml;
}

function _numLitXml(values) {
    const arr = Array.isArray(values) ? values : [];
    let xml = `<c:numLit xmlns:c="${C}"><c:formatCode>General</c:formatCode><c:ptCount val="${arr.length}"/>`;
    for (let i = 0; i < arr.length; i++) {
        xml += `<c:pt idx="${i}"><c:v>${Number.isFinite(+arr[i]) ? +arr[i] : 0}</c:v></c:pt>`;
    }
    xml += '</c:numLit>';
    return xml;
}

function _escXml(s) {
    return String(s == null ? '' : s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

function _axisXml(catAxPos, valAxPos, hasGridlines) {
    return `<c:catAx xmlns:c="${C}"><c:axId val="1"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="${catAxPos}"/><c:tickLblPos val="nextTo"/><c:crossAx val="2"/><c:crosses val="autoZero"/><c:auto val="1"/></c:catAx>` +
        `<c:valAx xmlns:c="${C}"><c:axId val="2"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="${valAxPos}"/>${hasGridlines ? '<c:majorGridlines/>' : ''}<c:numFmt formatCode="General" sourceLinked="1"/><c:tickLblPos val="nextTo"/><c:crossAx val="1"/><c:crosses val="autoZero"/></c:valAx>`;
}

function generatePieChartXml(opts) {
    const { title = '', categories = [], values = [], colors } = opts;
    const fills = colors || categories.map((_, i) => CHART_COLORS[i % CHART_COLORS.length]);

    let dPts = '';
    for (let i = 0; i < fills.length; i++) {
        dPts += `<c:dPt><c:idx val="${i}"/><c:spPr><a:solidFill><a:srgbClr val="${fills[i]}"/></a:solidFill></c:spPr></c:dPt>`;
    }

    return `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
        `<c:chartSpace xmlns:c="${C}" xmlns:a="${A}" xmlns:r="${R}">` +
        `<c:chart><c:plotArea><c:layout/>` +
        `<c:pieChart><c:varyColors val="1"/>` +
        `<c:ser><c:idx val="0"/><c:order val="0"/>` +
        `<c:tx><c:v>${_escXml(title)}</c:v></c:tx>` +
        dPts +
        `<c:cat>${_strLitXml(categories)}</c:cat>` +
        `<c:val>${_numLitXml(values)}</c:val>` +
        `</c:ser><c:firstSliceAng val="0"/></c:pieChart>` +
        `</c:plotArea>` +
        `<c:legend><c:legendPos val="r"/><c:overlay val="0"/></c:legend>` +
        `<c:plotVisOnly val="1"/></c:chart></c:chartSpace>`;
}

function generateBarChartXml(opts) {
    const { series = [], direction = 'col', grouping = 'clustered' } = opts;
    const isHorizontal = direction === 'bar';
    // Accept top-level shared x-axis labels — LLMs frequently write
    // `{ series: [{name, values}], labels: [...] }` instead of repeating
    // `categories` inside each series. Either form should work.
    const sharedCats = opts.categories || opts.labels || opts.xAxis || [];

    let seriesXml = '';
    for (let i = 0; i < series.length; i++) {
        const s = series[i];
        const color = s.color || CHART_COLORS[i % CHART_COLORS.length];
        const cats = (Array.isArray(s.categories) && s.categories.length) ? s.categories : sharedCats;
        const vals = Array.isArray(s.values) ? s.values : [];
        seriesXml += `<c:ser><c:idx val="${i}"/><c:order val="${i}"/>` +
            `<c:tx><c:v>${_escXml(s.name || `Series ${i + 1}`)}</c:v></c:tx>` +
            `<c:spPr><a:solidFill><a:srgbClr val="${color}"/></a:solidFill></c:spPr>` +
            `<c:cat>${_strLitXml(cats)}</c:cat>` +
            `<c:val>${_numLitXml(vals)}</c:val>` +
            `</c:ser>`;
    }

    return `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
        `<c:chartSpace xmlns:c="${C}" xmlns:a="${A}" xmlns:r="${R}">` +
        `<c:chart><c:plotArea><c:layout/>` +
        `<c:barChart><c:barDir val="${direction}"/><c:grouping val="${grouping}"/>` +
        `<c:varyColors val="0"/>` +
        seriesXml +
        `<c:axId val="1"/><c:axId val="2"/></c:barChart>` +
        _axisXml(isHorizontal ? 'l' : 'b', isHorizontal ? 'b' : 'l', true) +
        `</c:plotArea>` +
        `<c:legend><c:legendPos val="b"/><c:overlay val="0"/></c:legend>` +
        `<c:plotVisOnly val="1"/></c:chart></c:chartSpace>`;
}

function generateLineChartXml(opts) {
    const { series = [] } = opts;
    const sharedCats = opts.categories || opts.labels || opts.xAxis || [];

    let seriesXml = '';
    for (let i = 0; i < series.length; i++) {
        const s = series[i];
        const color = s.color || CHART_COLORS[i % CHART_COLORS.length];
        const smooth = s.smooth ? '<c:smooth val="1"/>' : '';
        const cats = (Array.isArray(s.categories) && s.categories.length) ? s.categories : sharedCats;
        const vals = Array.isArray(s.values) ? s.values : [];
        seriesXml += `<c:ser><c:idx val="${i}"/><c:order val="${i}"/>` +
            `<c:tx><c:v>${_escXml(s.name || `Series ${i + 1}`)}</c:v></c:tx>` +
            `<c:spPr><a:ln w="28575"><a:solidFill><a:srgbClr val="${color}"/></a:solidFill></a:ln></c:spPr>` +
            `<c:marker><c:symbol val="circle"/><c:size val="5"/>` +
            `<c:spPr><a:solidFill><a:srgbClr val="${color}"/></a:solidFill></c:spPr></c:marker>` +
            smooth +
            `<c:cat>${_strLitXml(cats)}</c:cat>` +
            `<c:val>${_numLitXml(vals)}</c:val>` +
            `</c:ser>`;
    }

    return `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` +
        `<c:chartSpace xmlns:c="${C}" xmlns:a="${A}" xmlns:r="${R}">` +
        `<c:chart><c:plotArea><c:layout/>` +
        `<c:lineChart><c:grouping val="standard"/><c:varyColors val="0"/>` +
        seriesXml +
        `<c:marker val="1"/>` +
        `<c:axId val="1"/><c:axId val="2"/></c:lineChart>` +
        _axisXml('b', 'l', true) +
        `</c:plotArea>` +
        `<c:legend><c:legendPos val="b"/><c:overlay val="0"/></c:legend>` +
        `<c:plotVisOnly val="1"/></c:chart></c:chartSpace>`;
}

function _chartDrawingXml(rId, cx, cy, docPrId, name) {
    return `<w:r xmlns:w="${W}" xmlns:r="${R}" xmlns:wp="${WP}" xmlns:a="${A}" xmlns:c="${C}">` +
        `<w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0">` +
        `<wp:extent cx="${cx}" cy="${cy}"/>` +
        `<wp:effectExtent l="0" t="0" r="0" b="0"/>` +
        `<wp:docPr id="${docPrId}" name="${name}"/>` +
        `<wp:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect="1"/></wp:cNvGraphicFramePr>` +
        `<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart">` +
        `<c:chart r:id="${rId}"/>` +
        `</a:graphicData></a:graphic>` +
        `</wp:inline></w:drawing></w:r>`;
}

async function patchChart(zip, chartType, opts) {
    const n = _nextChartId();
    const chartPartName = `charts/chart${n}.xml`;
    const chartFullPath = `word/${chartPartName}`;

    let chartXml;
    switch (chartType) {
        case 'pieChart': chartXml = generatePieChartXml(opts); break;
        case 'barChart': chartXml = generateBarChartXml(opts); break;
        case 'lineChart': chartXml = generateLineChartXml(opts); break;
        default: throw new Error(`Unknown chart type: ${chartType}`);
    }

    zip.file(chartFullPath, chartXml);

    const ctDoc = await zip.file('[Content_Types].xml').async('string').then(parseXml);
    const override = ctDoc.createElementNS(CT, 'Override');
    override.setAttribute('PartName', `/${chartFullPath}`);
    override.setAttribute('ContentType', CHART_CT);
    ctDoc.documentElement.appendChild(override);
    zip.file('[Content_Types].xml', serXml(ctDoc));

    const docRelsPath = 'word/_rels/document.xml.rels';
    const relsDoc = await getOrCreate(zip, docRelsPath, EMPTY_RELS);
    const rId = addRel(relsDoc, CHART_REL, chartPartName);
    zip.file(docRelsPath, serXml(relsDoc));

    const cx = String(opts.widthEmu || 5486400);
    const cy = String(opts.heightEmu || 3429000);
    const docPrId = 200 + n;
    const chartName = `Chart${n}`;

    const drawingRun = _chartDrawingXml(rId, cx, cy, docPrId, chartName);

    const pXml = `<w:p xmlns:w="${W}" xmlns:r="${R}" xmlns:wp="${WP}" xmlns:a="${A}" xmlns:c="${C}">` +
        `<w:pPr><w:jc w:val="center"/></w:pPr>` +
        drawingRun + `</w:p>`;

    const captionText = opts.caption || '';
    const captionXml = captionText
        ? `<w:p xmlns:w="${W}"><w:pPr><w:jc w:val="center"/><w:pStyle w:val="Caption"/></w:pPr>` +
        `<w:r><w:rPr><w:sz w:val="18"/></w:rPr><w:t xml:space="preserve">${_escXml(captionText)}</w:t></w:r></w:p>`
        : '';

    const docDoc = await zip.file('word/document.xml').async('string').then(parseXml);
    const body = docDoc.getElementsByTagNameNS(W, 'body')[0];
    if (!body) return;

    const paragraphs = [];
    for (let child = body.firstChild; child; child = child.nextSibling) {
        if (child.nodeType === 1 && (child.localName === 'p' || child.localName === 'tbl')) {
            paragraphs.push(child);
        }
    }

    const chartP = parseXml(pXml).documentElement;
    const imported = docDoc.importNode(chartP, true);

    function _importCaption() {
        if (!captionXml) return null;
        return docDoc.importNode(parseXml(captionXml).documentElement, true);
    }

    function _insertBeforeRef(refNode) {
        body.insertBefore(imported, refNode);
        const cap = _importCaption();
        if (cap) body.insertBefore(cap, refNode);
    }

    function _appendToEnd() {
        const sectPrs = docDoc.getElementsByTagNameNS(W, 'sectPr');
        const lastSect = sectPrs.length > 0 ? sectPrs[sectPrs.length - 1] : null;
        if (lastSect && lastSect.parentNode === body) {
            _insertBeforeRef(lastSect);
        } else {
            body.appendChild(imported);
            const cap = _importCaption();
            if (cap) body.appendChild(cap);
        }
    }

    const phId = opts.placeholderId;
    const idx = opts.paragraphIndex;

    if (phId) {
        const marker = `{{CHART:${phId}}}`;
        let found = false;
        for (let i = 0; i < paragraphs.length; i++) {
            if ((paragraphs[i].textContent || '').includes(marker)) {
                body.insertBefore(imported, paragraphs[i]);
                const cap = _importCaption();
                if (cap) body.insertBefore(cap, paragraphs[i]);
                body.removeChild(paragraphs[i]);
                found = true;
                break;
            }
        }
        if (!found) _appendToEnd();
    } else if (idx !== undefined && idx < paragraphs.length) {
        const refNode = paragraphs[idx];
        body.insertBefore(imported, refNode.nextSibling);
        const cap = _importCaption();
        if (cap) body.insertBefore(cap, imported.nextSibling);
    } else {
        _appendToEnd();
    }

    zip.file('word/document.xml', serXml(docDoc));
}


// ══════════════════════════════════════════════════════════════════
// Public API
// ══════════════════════════════════════════════════════════════════

/**
 * Apply patches to a docx buffer (from Packer.toBuffer()).
 * Returns patched buffer. Use in the same script as docx-js generation.
 *
 * @param {Buffer} buffer - docx buffer from Packer.toBuffer()
 * @param {Array} patches - array of patch configs
 * @returns {Promise<Buffer>}
 */
async function applyPatches(buffer, patches) {
    const zip = await JSZip.loadAsync(buffer);

    for (const p of patches) {
        switch (p.type) {
            case 'watermark':
                await patchWatermark(zip, p);
                break;
            case 'dropCap':
                await patchDropCap(zip, p);
                break;
            case 'stripe':
                await patchStripeTables(zip, p);
                break;
            case 'backgroundImage':
                await patchBackgroundImage(zip, p);
                break;
            case 'coverColor':
                await patchCoverColor(zip, p);
                break;
            case 'pieChart':
            case 'barChart':
            case 'lineChart':
                await patchChart(zip, p.type, p);
                break;
            default:
                throw new Error(`Unknown patch type: ${p.type}`);
        }
    }

    return zip.generateAsync({ type: 'nodebuffer', compression: 'DEFLATE' });
}

/**
 * Apply patches to an existing docx file (in-place).
 *
 * @param {string} docxPath
 * @param {Array} patches
 */
async function patchFile(docxPath, patches) {
    const buffer = fs.readFileSync(docxPath);
    const patched = await applyPatches(buffer, patches);
    fs.writeFileSync(docxPath, patched);
}

module.exports = { applyPatches, patchFile };


// ── CLI ───────────────────────────────────────────────────────────

if (require.main === module) {
    const args = process.argv.slice(2);
    if (args.length < 2) {
        console.error('用法: node docx_patches.js <file.docx> <patch_type> [args] [--option=value ...]');
        console.error('patch_type: watermark | stripe | dropCap | backgroundImage | coverColor | pieChart | barChart | lineChart');
        process.exit(1);
    }

    const [file, type, ...rest] = args;
    const opts = { type };
    for (const arg of rest) {
        if (arg.startsWith('--')) {
            const [k, v] = arg.slice(2).split('=');
            opts[k] = v;
        } else if (!opts.text && type === 'watermark') {
            opts.text = arg;
        } else if (!opts.imagePath && type === 'backgroundImage') {
            opts.imagePath = arg;
        }
    }
    if (opts.opacity) opts.opacity = parseFloat(opts.opacity);
    if (opts.rotation) opts.rotation = parseInt(opts.rotation, 10);
    if (opts.lines) opts.lines = parseInt(opts.lines, 10);
    if (opts.paragraphIndex) opts.paragraphIndex = parseInt(opts.paragraphIndex, 10);

    patchFile(file, [opts]).then(() => {
        console.log(`[docx_patches] ${type} applied to ${file}`);
    }).catch(err => {
        console.error(err.message);
        process.exit(1);
    });
}
