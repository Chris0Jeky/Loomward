/**
 * Shared JSDoc types for the viz modules. Types only: this file exports nothing at runtime.
 *
 * The slice types mirror the subset of contracts/v3 `TreeSlice` that the renderers read, written
 * structurally so the shell's hand-written types and the generated contract types both satisfy them.
 * Fields the contract is adding (PR #110: `aggregate_state`) are optional here so either version fits.
 */

/** @typedef {'labelled' | 'suggested' | 'mixed' | 'none' | 'pending' | 'unknown'} MeaningState */
/** @typedef {'granted' | 'partial' | 'excluded' | 'denied' | 'revoked' | 'unknown'} PermissionState */

/**
 * @typedef {object} ThreadsLike
 * @property {{ state: MeaningState, label: string | null, share?: number | null, source?: string | null, collection_ids?: string[] }} meaning
 * @property {{ volume_id: string | null, tier: number | null, tier_basis: string }} residency
 * @property {{ state: PermissionState, reason: string | null }} permission
 */

/**
 * @typedef {object} SliceNodeLike
 * @property {string} node_id
 * @property {number | null} parent    index into the slice's nodes; parents precede children
 * @property {string} kind             'atlas' | 'volume' | 'root' | 'dir' | 'file' | 'other'
 * @property {string} name
 * @property {number} depth
 * @property {string} size_bytes       exact decimal string under the slice's basis
 * @property {number} size_unknown_files
 * @property {string} logical_bytes
 * @property {string | null} allocated_bytes
 * @property {number} files
 * @property {number} dirs
 * @property {number | null} child_count
 * @property {number | null} folded_count
 * @property {string} coverage
 * @property {boolean} live
 * @property {string | null} [ext_family]
 * @property {string | null} [modified_at]
 * @property {ThreadsLike} threads
 */

/**
 * @typedef {object} SliceLike
 * @property {string} anchor_node_id
 * @property {'logical' | 'allocated'} basis
 * @property {boolean} complete
 * @property {boolean} truncated
 * @property {boolean} [live]
 * @property {'consistent' | 'provisional_live'} [aggregate_state]
 * @property {SliceNodeLike[]} nodes
 */

/**
 * Colours and faces, read from the CSS tokens by the host. Renderers never hard-code a colour.
 * @typedef {object} Palette
 * @property {string} ink          cloth background
 * @property {string} frame        container fill
 * @property {string} head         container label band
 * @property {string} scrim        label patch behind text on cloth
 * @property {string} label
 * @property {string} labelDim
 * @property {string} fringe       folded "smaller items" hatch
 * @property {string} loose        unwoven threads: bytes whose contents are not in the slice
 * @property {string} cursor
 * @property {string} hover
 * @property {string} shuttle
 * @property {string} track        sunburst ring track
 * @property {string} scale        sunburst outer scale
 * @property {string[]} dyes       warp: meaning dyes (at least 2)
 * @property {string} undyed       warp: no meaning
 * @property {string} neutral      a thread whose channel is switched off
 * @property {[string, string, string]} metals  weft: residency tiers hot, warm, cold
 * @property {string} hollow       weft: cloud placeholder (nothing resident)
 * @property {string} permission   selvedge
 * @property {string} unknown      unknown marks (dotted stitch, missing weft)
 * @property {string} fontDisplay
 * @property {string} fontHead
 * @property {string} fontLabel
 * @property {string} fontNum
 * @property {string} fontSmall
 */

/**
 * Which threads are shown; a hidden thread is drawn neutral, never as "unknown".
 * @typedef {{ meaning: boolean, residency: boolean, permission: boolean }} ThreadToggles
 */

/**
 * What the renderers report about a node in events and to the inspector.
 * @typedef {object} NodeInfo
 * @property {string} id
 * @property {string} name                 the raw name: show it through the host's hidden-character handling
 * @property {string} label                the escaped name the canvas draws
 * @property {number} size                 lossy number for layout and shares
 * @property {SliceNodeLike | null} src    the slice node, or null for a synthetic cell
 * @property {'remainder' | 'fold' | null} synthetic
 * @property {number | null} folded       count folded into a "smaller" cell (0 otherwise, null when unknown)
 * @property {SliceNodeLike[]} zero        children with no area: zero bytes, denied, unmeasured
 * @property {string | null} parentId
 * @property {boolean} drillable
 */

/**
 * @typedef {'select' | 'hover' | 'drill' | 'back'} VizEvent
 */

export {};
