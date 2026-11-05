//! TRANREPT [app/jcl/TRANREPT.jcl] with its JCL default dates over the fixture corpus, against
//! the parity audit (AAP 0.9.3): `batch posttran` posts `dailytran.txt` with the processing
//! clock pinned to [`TRANREPT_CLOCK`], so every posted `TRAN-PROC-TS` falls inside the default
//! range `2022-01-01` to `2022-07-06`, and `batch tranrept` then writes one generation of each
//! of its three datasets:
//!
//! - `TRANSACT.BKUP(+1)`, the `REPROC` copy of the live master (P-TRANREPT-001): 350-byte
//!   records in TRAN-ID order whose bytes 1-330 are the live master rows encoded through the
//!   codec `TransactionLayout` (the FILLER is not compared, E-25), holding the records POSTTRAN
//!   posted (P-CBTRN02C-216) with the pinned processing timestamp (P-CBTRN02C-220).
//! - `TRANSACT.DALY(+1)`, the `SORT` output (P-TRANREPT-002): the backup records whose
//!   `TRAN-PROC-DT` (bytes 305-314) lies in the range, whole and in ascending card-number order,
//!   in exactly the order the detail lines of audit D.6 give.
//! - `TRANREPT(+1)`, the CBTRN03C report (P-TRANREPT-003): 133-byte lines equal to the D.6
//!   lines P-CBTRN03C-016 to P-CBTRN03C-466 in order and count, headers, details, page totals,
//!   account totals and the grand total included, with the end-of-input behaviour of
//!   P-CBTRN03C-009 (the last amount added again, the last account total never written).
//!
//! Every expected value is read from `docs/parity/parity-oracle-audit.md` through
//! `carddemo_audit`; a missing row or key fails the test instead of being guessed. The
//! descriptions on the detail lines come from the `TRANTYPE` and `TRANCATG` processing copies
//! the seed loads, never from the Db2 masters (P-CBTRN03C-001), so the lines show the fixture's
//! `Purchase` where the Db2 initial load holds `PURCHASE`.
//!
//! Each file is written by the real `batch` binary, each record or line followed by LF, under
//! the harness's own `CARDDEMO_BATCH_DIR`.

pub mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;
use std::ops::Range;
use std::str::FromStr;

use carddemo_audit::{
    CodeSpan, Document, KeyValue, PARITY_COMMON_COLUMNS, PARITY_LAYOUT_COLUMNS, Table,
    cell_code_spans, code_span, key_values,
};
use carddemo_codec::layouts::{TransactionLayout, TransactionRecord};
use carddemo_codec::{Charset, Layout};
use carddemo_domain::entities::{NewTransaction, Transaction};
use carddemo_domain::numeric::{Alnum, CardNumber, Money, TranId, TranTypeCode, TransactionRowId};
use carddemo_domain::rules::transaction_report::ReportRun;
use common::{
    Harness, LIVE_MASTER_COLUMNS, TRANREPT_CLOCK, block_on, expected_span, expected_value,
    fixture_lines, parity, parity_cell, parity_row, split_records,
};
use rust_decimal::Decimal;

/// Every parity audit row this test proves. The coverage check
/// `parity_audit_coverage.rs` reads the literal IDs, and the test checks that each one exists
/// and that every row of audit D.6 is listed.
const P_IDS: &[&str] = &[
    // Section B, TRANREPT: the REPROC backup, the SORT selection and the CBTRN03C step.
    "P-TRANREPT-001",
    "P-TRANREPT-002",
    "P-TRANREPT-003",
    // Section B, CBTRN03C: the paths a normal run over the sorted input takes, which the report
    // lines prove: the file opens with TRANREPT X(133), the DATEPARM dates, the sequential read,
    // the card break, the header block and page break, the detail line, every write, the page
    // and account totals, the end of input and the grand total.
    "P-CBTRN03C-001",
    "P-CBTRN03C-002",
    "P-CBTRN03C-474",
    "P-CBTRN03C-004",
    "P-CBTRN03C-006",
    "P-CBTRN03C-475",
    "P-CBTRN03C-007",
    "P-CBTRN03C-476",
    "P-CBTRN03C-008",
    "P-CBTRN03C-009",
    "P-CBTRN03C-477",
    // Section C, CBTRN03C: the totals arithmetic and the two edited amount pictures.
    "P-CBTRN03C-010",
    "P-CBTRN03C-011",
    "P-CBTRN03C-486",
    "P-CBTRN03C-012",
    "P-CBTRN03C-013",
    "P-CBTRN03C-014",
    // Section D.2, POSTTRAN: the posted records the backup holds.
    "P-CBTRN02C-216",
    // Section E: the DATEPARM dates (E-34), the order within one card (E-38) and the FILLER
    // bytes no program sets (E-25).
    "P-CBTRN03C-467",
    "P-COMBTRAN-009",
    "P-CVTRA05Y-018",
    // Section F: the pinned processing timestamp POSTTRAN stamps.
    "P-CBTRN02C-220",
    // Section D.6, TRANREPT: P-CBTRN03C-015 is the summary row, P-CBTRN03C-016 to
    // P-CBTRN03C-466 are the report lines 1 to 451.
    "P-CBTRN03C-015",
    "P-CBTRN03C-016",
    "P-CBTRN03C-017",
    "P-CBTRN03C-018",
    "P-CBTRN03C-019",
    "P-CBTRN03C-020",
    "P-CBTRN03C-021",
    "P-CBTRN03C-022",
    "P-CBTRN03C-023",
    "P-CBTRN03C-024",
    "P-CBTRN03C-025",
    "P-CBTRN03C-026",
    "P-CBTRN03C-027",
    "P-CBTRN03C-028",
    "P-CBTRN03C-029",
    "P-CBTRN03C-030",
    "P-CBTRN03C-031",
    "P-CBTRN03C-032",
    "P-CBTRN03C-033",
    "P-CBTRN03C-034",
    "P-CBTRN03C-035",
    "P-CBTRN03C-036",
    "P-CBTRN03C-037",
    "P-CBTRN03C-038",
    "P-CBTRN03C-039",
    "P-CBTRN03C-040",
    "P-CBTRN03C-041",
    "P-CBTRN03C-042",
    "P-CBTRN03C-043",
    "P-CBTRN03C-044",
    "P-CBTRN03C-045",
    "P-CBTRN03C-046",
    "P-CBTRN03C-047",
    "P-CBTRN03C-048",
    "P-CBTRN03C-049",
    "P-CBTRN03C-050",
    "P-CBTRN03C-051",
    "P-CBTRN03C-052",
    "P-CBTRN03C-053",
    "P-CBTRN03C-054",
    "P-CBTRN03C-055",
    "P-CBTRN03C-056",
    "P-CBTRN03C-057",
    "P-CBTRN03C-058",
    "P-CBTRN03C-059",
    "P-CBTRN03C-060",
    "P-CBTRN03C-061",
    "P-CBTRN03C-062",
    "P-CBTRN03C-063",
    "P-CBTRN03C-064",
    "P-CBTRN03C-065",
    "P-CBTRN03C-066",
    "P-CBTRN03C-067",
    "P-CBTRN03C-068",
    "P-CBTRN03C-069",
    "P-CBTRN03C-070",
    "P-CBTRN03C-071",
    "P-CBTRN03C-072",
    "P-CBTRN03C-073",
    "P-CBTRN03C-074",
    "P-CBTRN03C-075",
    "P-CBTRN03C-076",
    "P-CBTRN03C-077",
    "P-CBTRN03C-078",
    "P-CBTRN03C-079",
    "P-CBTRN03C-080",
    "P-CBTRN03C-081",
    "P-CBTRN03C-082",
    "P-CBTRN03C-083",
    "P-CBTRN03C-084",
    "P-CBTRN03C-085",
    "P-CBTRN03C-086",
    "P-CBTRN03C-087",
    "P-CBTRN03C-088",
    "P-CBTRN03C-089",
    "P-CBTRN03C-090",
    "P-CBTRN03C-091",
    "P-CBTRN03C-092",
    "P-CBTRN03C-093",
    "P-CBTRN03C-094",
    "P-CBTRN03C-095",
    "P-CBTRN03C-096",
    "P-CBTRN03C-097",
    "P-CBTRN03C-098",
    "P-CBTRN03C-099",
    "P-CBTRN03C-100",
    "P-CBTRN03C-101",
    "P-CBTRN03C-102",
    "P-CBTRN03C-103",
    "P-CBTRN03C-104",
    "P-CBTRN03C-105",
    "P-CBTRN03C-106",
    "P-CBTRN03C-107",
    "P-CBTRN03C-108",
    "P-CBTRN03C-109",
    "P-CBTRN03C-110",
    "P-CBTRN03C-111",
    "P-CBTRN03C-112",
    "P-CBTRN03C-113",
    "P-CBTRN03C-114",
    "P-CBTRN03C-115",
    "P-CBTRN03C-116",
    "P-CBTRN03C-117",
    "P-CBTRN03C-118",
    "P-CBTRN03C-119",
    "P-CBTRN03C-120",
    "P-CBTRN03C-121",
    "P-CBTRN03C-122",
    "P-CBTRN03C-123",
    "P-CBTRN03C-124",
    "P-CBTRN03C-125",
    "P-CBTRN03C-126",
    "P-CBTRN03C-127",
    "P-CBTRN03C-128",
    "P-CBTRN03C-129",
    "P-CBTRN03C-130",
    "P-CBTRN03C-131",
    "P-CBTRN03C-132",
    "P-CBTRN03C-133",
    "P-CBTRN03C-134",
    "P-CBTRN03C-135",
    "P-CBTRN03C-136",
    "P-CBTRN03C-137",
    "P-CBTRN03C-138",
    "P-CBTRN03C-139",
    "P-CBTRN03C-140",
    "P-CBTRN03C-141",
    "P-CBTRN03C-142",
    "P-CBTRN03C-143",
    "P-CBTRN03C-144",
    "P-CBTRN03C-145",
    "P-CBTRN03C-146",
    "P-CBTRN03C-147",
    "P-CBTRN03C-148",
    "P-CBTRN03C-149",
    "P-CBTRN03C-150",
    "P-CBTRN03C-151",
    "P-CBTRN03C-152",
    "P-CBTRN03C-153",
    "P-CBTRN03C-154",
    "P-CBTRN03C-155",
    "P-CBTRN03C-156",
    "P-CBTRN03C-157",
    "P-CBTRN03C-158",
    "P-CBTRN03C-159",
    "P-CBTRN03C-160",
    "P-CBTRN03C-161",
    "P-CBTRN03C-162",
    "P-CBTRN03C-163",
    "P-CBTRN03C-164",
    "P-CBTRN03C-165",
    "P-CBTRN03C-166",
    "P-CBTRN03C-167",
    "P-CBTRN03C-168",
    "P-CBTRN03C-169",
    "P-CBTRN03C-170",
    "P-CBTRN03C-171",
    "P-CBTRN03C-172",
    "P-CBTRN03C-173",
    "P-CBTRN03C-174",
    "P-CBTRN03C-175",
    "P-CBTRN03C-176",
    "P-CBTRN03C-177",
    "P-CBTRN03C-178",
    "P-CBTRN03C-179",
    "P-CBTRN03C-180",
    "P-CBTRN03C-181",
    "P-CBTRN03C-182",
    "P-CBTRN03C-183",
    "P-CBTRN03C-184",
    "P-CBTRN03C-185",
    "P-CBTRN03C-186",
    "P-CBTRN03C-187",
    "P-CBTRN03C-188",
    "P-CBTRN03C-189",
    "P-CBTRN03C-190",
    "P-CBTRN03C-191",
    "P-CBTRN03C-192",
    "P-CBTRN03C-193",
    "P-CBTRN03C-194",
    "P-CBTRN03C-195",
    "P-CBTRN03C-196",
    "P-CBTRN03C-197",
    "P-CBTRN03C-198",
    "P-CBTRN03C-199",
    "P-CBTRN03C-200",
    "P-CBTRN03C-201",
    "P-CBTRN03C-202",
    "P-CBTRN03C-203",
    "P-CBTRN03C-204",
    "P-CBTRN03C-205",
    "P-CBTRN03C-206",
    "P-CBTRN03C-207",
    "P-CBTRN03C-208",
    "P-CBTRN03C-209",
    "P-CBTRN03C-210",
    "P-CBTRN03C-211",
    "P-CBTRN03C-212",
    "P-CBTRN03C-213",
    "P-CBTRN03C-214",
    "P-CBTRN03C-215",
    "P-CBTRN03C-216",
    "P-CBTRN03C-217",
    "P-CBTRN03C-218",
    "P-CBTRN03C-219",
    "P-CBTRN03C-220",
    "P-CBTRN03C-221",
    "P-CBTRN03C-222",
    "P-CBTRN03C-223",
    "P-CBTRN03C-224",
    "P-CBTRN03C-225",
    "P-CBTRN03C-226",
    "P-CBTRN03C-227",
    "P-CBTRN03C-228",
    "P-CBTRN03C-229",
    "P-CBTRN03C-230",
    "P-CBTRN03C-231",
    "P-CBTRN03C-232",
    "P-CBTRN03C-233",
    "P-CBTRN03C-234",
    "P-CBTRN03C-235",
    "P-CBTRN03C-236",
    "P-CBTRN03C-237",
    "P-CBTRN03C-238",
    "P-CBTRN03C-239",
    "P-CBTRN03C-240",
    "P-CBTRN03C-241",
    "P-CBTRN03C-242",
    "P-CBTRN03C-243",
    "P-CBTRN03C-244",
    "P-CBTRN03C-245",
    "P-CBTRN03C-246",
    "P-CBTRN03C-247",
    "P-CBTRN03C-248",
    "P-CBTRN03C-249",
    "P-CBTRN03C-250",
    "P-CBTRN03C-251",
    "P-CBTRN03C-252",
    "P-CBTRN03C-253",
    "P-CBTRN03C-254",
    "P-CBTRN03C-255",
    "P-CBTRN03C-256",
    "P-CBTRN03C-257",
    "P-CBTRN03C-258",
    "P-CBTRN03C-259",
    "P-CBTRN03C-260",
    "P-CBTRN03C-261",
    "P-CBTRN03C-262",
    "P-CBTRN03C-263",
    "P-CBTRN03C-264",
    "P-CBTRN03C-265",
    "P-CBTRN03C-266",
    "P-CBTRN03C-267",
    "P-CBTRN03C-268",
    "P-CBTRN03C-269",
    "P-CBTRN03C-270",
    "P-CBTRN03C-271",
    "P-CBTRN03C-272",
    "P-CBTRN03C-273",
    "P-CBTRN03C-274",
    "P-CBTRN03C-275",
    "P-CBTRN03C-276",
    "P-CBTRN03C-277",
    "P-CBTRN03C-278",
    "P-CBTRN03C-279",
    "P-CBTRN03C-280",
    "P-CBTRN03C-281",
    "P-CBTRN03C-282",
    "P-CBTRN03C-283",
    "P-CBTRN03C-284",
    "P-CBTRN03C-285",
    "P-CBTRN03C-286",
    "P-CBTRN03C-287",
    "P-CBTRN03C-288",
    "P-CBTRN03C-289",
    "P-CBTRN03C-290",
    "P-CBTRN03C-291",
    "P-CBTRN03C-292",
    "P-CBTRN03C-293",
    "P-CBTRN03C-294",
    "P-CBTRN03C-295",
    "P-CBTRN03C-296",
    "P-CBTRN03C-297",
    "P-CBTRN03C-298",
    "P-CBTRN03C-299",
    "P-CBTRN03C-300",
    "P-CBTRN03C-301",
    "P-CBTRN03C-302",
    "P-CBTRN03C-303",
    "P-CBTRN03C-304",
    "P-CBTRN03C-305",
    "P-CBTRN03C-306",
    "P-CBTRN03C-307",
    "P-CBTRN03C-308",
    "P-CBTRN03C-309",
    "P-CBTRN03C-310",
    "P-CBTRN03C-311",
    "P-CBTRN03C-312",
    "P-CBTRN03C-313",
    "P-CBTRN03C-314",
    "P-CBTRN03C-315",
    "P-CBTRN03C-316",
    "P-CBTRN03C-317",
    "P-CBTRN03C-318",
    "P-CBTRN03C-319",
    "P-CBTRN03C-320",
    "P-CBTRN03C-321",
    "P-CBTRN03C-322",
    "P-CBTRN03C-323",
    "P-CBTRN03C-324",
    "P-CBTRN03C-325",
    "P-CBTRN03C-326",
    "P-CBTRN03C-327",
    "P-CBTRN03C-328",
    "P-CBTRN03C-329",
    "P-CBTRN03C-330",
    "P-CBTRN03C-331",
    "P-CBTRN03C-332",
    "P-CBTRN03C-333",
    "P-CBTRN03C-334",
    "P-CBTRN03C-335",
    "P-CBTRN03C-336",
    "P-CBTRN03C-337",
    "P-CBTRN03C-338",
    "P-CBTRN03C-339",
    "P-CBTRN03C-340",
    "P-CBTRN03C-341",
    "P-CBTRN03C-342",
    "P-CBTRN03C-343",
    "P-CBTRN03C-344",
    "P-CBTRN03C-345",
    "P-CBTRN03C-346",
    "P-CBTRN03C-347",
    "P-CBTRN03C-348",
    "P-CBTRN03C-349",
    "P-CBTRN03C-350",
    "P-CBTRN03C-351",
    "P-CBTRN03C-352",
    "P-CBTRN03C-353",
    "P-CBTRN03C-354",
    "P-CBTRN03C-355",
    "P-CBTRN03C-356",
    "P-CBTRN03C-357",
    "P-CBTRN03C-358",
    "P-CBTRN03C-359",
    "P-CBTRN03C-360",
    "P-CBTRN03C-361",
    "P-CBTRN03C-362",
    "P-CBTRN03C-363",
    "P-CBTRN03C-364",
    "P-CBTRN03C-365",
    "P-CBTRN03C-366",
    "P-CBTRN03C-367",
    "P-CBTRN03C-368",
    "P-CBTRN03C-369",
    "P-CBTRN03C-370",
    "P-CBTRN03C-371",
    "P-CBTRN03C-372",
    "P-CBTRN03C-373",
    "P-CBTRN03C-374",
    "P-CBTRN03C-375",
    "P-CBTRN03C-376",
    "P-CBTRN03C-377",
    "P-CBTRN03C-378",
    "P-CBTRN03C-379",
    "P-CBTRN03C-380",
    "P-CBTRN03C-381",
    "P-CBTRN03C-382",
    "P-CBTRN03C-383",
    "P-CBTRN03C-384",
    "P-CBTRN03C-385",
    "P-CBTRN03C-386",
    "P-CBTRN03C-387",
    "P-CBTRN03C-388",
    "P-CBTRN03C-389",
    "P-CBTRN03C-390",
    "P-CBTRN03C-391",
    "P-CBTRN03C-392",
    "P-CBTRN03C-393",
    "P-CBTRN03C-394",
    "P-CBTRN03C-395",
    "P-CBTRN03C-396",
    "P-CBTRN03C-397",
    "P-CBTRN03C-398",
    "P-CBTRN03C-399",
    "P-CBTRN03C-400",
    "P-CBTRN03C-401",
    "P-CBTRN03C-402",
    "P-CBTRN03C-403",
    "P-CBTRN03C-404",
    "P-CBTRN03C-405",
    "P-CBTRN03C-406",
    "P-CBTRN03C-407",
    "P-CBTRN03C-408",
    "P-CBTRN03C-409",
    "P-CBTRN03C-410",
    "P-CBTRN03C-411",
    "P-CBTRN03C-412",
    "P-CBTRN03C-413",
    "P-CBTRN03C-414",
    "P-CBTRN03C-415",
    "P-CBTRN03C-416",
    "P-CBTRN03C-417",
    "P-CBTRN03C-418",
    "P-CBTRN03C-419",
    "P-CBTRN03C-420",
    "P-CBTRN03C-421",
    "P-CBTRN03C-422",
    "P-CBTRN03C-423",
    "P-CBTRN03C-424",
    "P-CBTRN03C-425",
    "P-CBTRN03C-426",
    "P-CBTRN03C-427",
    "P-CBTRN03C-428",
    "P-CBTRN03C-429",
    "P-CBTRN03C-430",
    "P-CBTRN03C-431",
    "P-CBTRN03C-432",
    "P-CBTRN03C-433",
    "P-CBTRN03C-434",
    "P-CBTRN03C-435",
    "P-CBTRN03C-436",
    "P-CBTRN03C-437",
    "P-CBTRN03C-438",
    "P-CBTRN03C-439",
    "P-CBTRN03C-440",
    "P-CBTRN03C-441",
    "P-CBTRN03C-442",
    "P-CBTRN03C-443",
    "P-CBTRN03C-444",
    "P-CBTRN03C-445",
    "P-CBTRN03C-446",
    "P-CBTRN03C-447",
    "P-CBTRN03C-448",
    "P-CBTRN03C-449",
    "P-CBTRN03C-450",
    "P-CBTRN03C-451",
    "P-CBTRN03C-452",
    "P-CBTRN03C-453",
    "P-CBTRN03C-454",
    "P-CBTRN03C-455",
    "P-CBTRN03C-456",
    "P-CBTRN03C-457",
    "P-CBTRN03C-458",
    "P-CBTRN03C-459",
    "P-CBTRN03C-460",
    "P-CBTRN03C-461",
    "P-CBTRN03C-462",
    "P-CBTRN03C-463",
    "P-CBTRN03C-464",
    "P-CBTRN03C-465",
    "P-CBTRN03C-466",
];

/// The condition code of a job that ends normally.
const SUCCESS: i32 = 0;

/// `RETURN-CODE` 4, which CBTRN02C sets when it wrote at least one reject
/// [app/cbl/CBTRN02C.cbl:L229-231]: the fixture corpus holds the rejects of audit D.1.
const RECORDS_REJECTED: i32 = 4;

/// `TRANREPT(+1)` `DCB=(LRECL=133,RECFM=FB)` [app/jcl/TRANREPT.jcl:L76-80].
const REPORT_LRECL: usize = 133;

/// The D.6 summary row: the selection range, the record and line counts, the width and the
/// terminator of `TRANREPT(+1)`.
const SUMMARY: &str = "P-CBTRN03C-015";

/// The D.2 row of the records POSTTRAN writes to the live master.
const POSTED: &str = "P-CBTRN02C-216";

/// The section F row of the processing timestamp POSTTRAN stamps under the pinned clock.
const PINNED_PROC_TS: &str = "P-CBTRN02C-220";

/// The `###` heading of the TRANREPT fixture expectations.
const D6_SUBSECTION: &str = "D.6 TRANREPT";

/// The `kind` of a D.6 detail line, the one kind that carries a `tran_id`.
const DETAIL_KIND: &str = "TRANSACTION-DETAIL-REPORT";

/// The header of the column holding each audit row's structured result.
const EXPECTED_RESULT: &str = PARITY_COMMON_COLUMNS[5];

/// `TRAN-ID X(16)`, offset 0 [app/cpy/CVTRA05Y.cpy:L5].
const TRAN_ID: Range<usize> = 0..16;

/// `TRAN-CARD-NUM`, the sort key `TRAN-CARD-NUM,263,16,ZD` [app/jcl/TRANREPT.jcl:L41].
const TRAN_CARD_NUM: Range<usize> = 262..278;

/// `TRAN-PROC-DT`, the include field `TRAN-PROC-DT,305,10,CH` [app/jcl/TRANREPT.jcl:L42].
const TRAN_PROC_DT: Range<usize> = 304..314;

/// `TRAN-PROC-TS X(26)`, offset 304 [app/cpy/CVTRA05Y.cpy:L17].
const TRAN_PROC_TS: Range<usize> = 304..330;

/// `TRAN-ID` through `TRAN-PROC-TS`, every byte of the record before its `FILLER X(20)`
/// [app/cpy/CVTRA05Y.cpy:L5-18].
const RECORD_CONTENT: Range<usize> = 0..330;

#[test]
fn tranrept_defaults_match_audit() {
    let doc = parity();
    assert_rows_exist_and_d6_is_cited(&doc);
    let audit = Expected::from_audit(&doc);

    let h = Harness::new();
    let _ = h.run_expect(&["posttran"], Some(TRANREPT_CLOCK), RECORDS_REJECTED);
    // No date arguments: the JCL dates `PARM-START-DATE` and `PARM-END-DATE`
    // [app/jcl/TRANREPT.jcl:L43-44] are the subcommand defaults.
    let _ = h.run_expect(&["tranrept"], Some(TRANREPT_CLOCK), SUCCESS);
    for dataset in ["TRANSACT.BKUP", "TRANSACT.DALY", "TRANREPT"] {
        assert_eq!(
            h.generations(dataset),
            [h.batch_dir().join(format!("{dataset}.G0001V00"))],
            "{dataset} must have exactly one generation, its first"
        );
    }

    let bkup = split_records(&h.newest("TRANSACT.BKUP"), TransactionLayout::LEN);
    check_backup(&audit, &h.live_master_rows(), &bkup);
    let daly = split_records(&h.newest("TRANSACT.DALY"), TransactionLayout::LEN);
    check_sorted_daily(&audit, &bkup, &daly);
    let report = split_records(&h.newest("TRANREPT"), REPORT_LRECL);
    assert_same_lines("TRANREPT(+1)", &audit.lines, &report);
}

/// One expected record or line, with the label a mismatch message names it by.
#[derive(Clone)]
struct Line {
    label: String,
    text: String,
}

/// The TRANREPT expectations of the parity audit.
struct Expected {
    /// The `start=` of the D.6 summary: `PARM-START-DATE` and the `DATEPARM` start date.
    start: String,
    /// The `end=` of the D.6 summary: `PARM-END-DATE` and the `DATEPARM` end date.
    end: String,
    /// `records_in_range=` of the D.6 summary: the records the sort keeps.
    records_in_range: usize,
    /// The report lines of D.6, in `line_no` order.
    lines: Vec<Line>,
    /// The `tran_id` of each D.6 detail line, in report order, labelled by its row ID: the
    /// order of `TRANSACT.DALY(+1)`, which CBTRN03C reads in file order.
    details: Vec<Line>,
    /// `posted=` of P-CBTRN02C-216: the records of the live master after POSTTRAN.
    posted: usize,
    /// `first=` of P-CBTRN02C-216: the first posted TRAN-ID in file order, also the lowest,
    /// because `dailytran.txt` is in ascending TRAN-ID order.
    first: String,
    /// `last=` of P-CBTRN02C-216: the last posted TRAN-ID in file order, also the highest.
    last: String,
    /// `pinned=` of P-CBTRN02C-220: the `TRAN-PROC-TS` of every posted record.
    proc_ts: String,
}

impl Expected {
    /// Reads the D.6 summary and lines, the posted records of P-CBTRN02C-216 and the pinned
    /// timestamp of P-CBTRN02C-220, and checks that they agree with one another and with the
    /// JCL record length.
    ///
    /// # Panics
    ///
    /// Panics, naming the row and the key, if a row or key is missing, a count is not a number,
    /// or the counts, the width or the terminator disagree.
    fn from_audit(doc: &Document) -> Self {
        let width = count(doc, SUMMARY, "width");
        assert_eq!(
            width, REPORT_LRECL,
            "{SUMMARY} width= against the TRANREPT(+1) LRECL"
        );
        assert_eq!(
            expected_value(doc, SUMMARY, "terminator"),
            "LF",
            "{SUMMARY} terminator="
        );
        let range = key_values(parity_cell(doc, SUMMARY, "Input condition"));
        let (lines, details) = report_lines(doc, width);
        assert_eq!(
            lines.len(),
            count(doc, SUMMARY, "lines"),
            "the D.6 report lines against {SUMMARY} lines="
        );
        let records_in_range = count(doc, SUMMARY, "records_in_range");
        assert_eq!(
            details.len(),
            records_in_range,
            "the D.6 detail lines against {SUMMARY} records_in_range="
        );
        Self {
            start: required(&range, "start", SUMMARY).to_owned(),
            end: required(&range, "end", SUMMARY).to_owned(),
            records_in_range,
            lines,
            details,
            posted: count(doc, POSTED, "posted"),
            first: expected_value(doc, POSTED, "first"),
            last: expected_value(doc, POSTED, "last"),
            proc_ts: unquote(
                &expected_span(doc, PINNED_PROC_TS, "pinned").text,
                PINNED_PROC_TS,
            ),
        }
    }
}

/// Fails unless every ID of [`P_IDS`] names a data row of the audit, once, and every row of
/// the D.6 table is among them, so a line added to the audit cannot go uncited.
fn assert_rows_exist_and_d6_is_cited(doc: &Document) {
    for id in P_IDS {
        let _ = parity_row(doc, id);
    }
    let cited: BTreeSet<&str> = P_IDS.iter().copied().collect();
    assert_eq!(cited.len(), P_IDS.len(), "P_IDS lists an ID twice");
    let table = d6_table(doc);
    let uncited: Vec<&str> = table
        .rows
        .iter()
        .filter_map(|row| table.cell(row, "ID"))
        .filter(|id| !cited.contains(id))
        .collect();
    assert!(
        uncited.is_empty(),
        "P_IDS lacks the {D6_SUBSECTION} rows {uncited:?}"
    );
}

/// The one data table under `### D.6 TRANREPT`.
///
/// # Panics
///
/// Panics unless the audit has exactly one such table.
fn d6_table(doc: &Document) -> &Table {
    let mut tables = doc.tables().iter().filter(|table| {
        table.is_data_table() && table.subsection.as_deref() == Some(D6_SUBSECTION)
    });
    let table = tables
        .next()
        .unwrap_or_else(|| panic!("the parity audit has no {D6_SUBSECTION} table"));
    assert!(
        tables.next().is_none(),
        "the parity audit has more than one {D6_SUBSECTION} table"
    );
    table
}

/// The report lines of the D.6 table in `line_no` order, each its exact text padded to
/// `width`, and the `tran_id` of each detail line among them, in the same order.
///
/// Rows without `line_no=`, the summary row, describe no line.
///
/// # Panics
///
/// Panics, naming the row, if the `line_no` values do not run 1, 2, 3, ... in table order,
/// or a line row lacks its `kind`, its exact `text`, or, for a detail line, its `tran_id`.
fn report_lines(doc: &Document, width: usize) -> (Vec<Line>, Vec<Line>) {
    let table = d6_table(doc);
    let mut lines = Vec::new();
    let mut details = Vec::new();
    for row in &table.rows {
        let id = table
            .cell(row, "ID")
            .unwrap_or_else(|| panic!("a {D6_SUBSECTION} row at line {} has no ID", row.line));
        let result = table
            .cell(row, EXPECTED_RESULT)
            .unwrap_or_else(|| panic!("{id} has no {EXPECTED_RESULT:?} cell"));
        let pairs = key_values(result);
        let Some(line_no) = pair(&pairs, "line_no") else {
            continue;
        };
        assert_eq!(
            line_no,
            (lines.len() + 1).to_string(),
            "{id}: the {D6_SUBSECTION} lines are not numbered in table order"
        );
        let kind = required(&pairs, "kind", id);
        let span = code_span(required(&pairs, "text", id))
            .unwrap_or_else(|| panic!("{id}: text= holds no exact text"));
        lines.push(Line {
            label: format!("{id} {kind}"),
            text: exact_text(&span, width, id),
        });
        if kind == DETAIL_KIND {
            details.push(Line {
                label: id.to_owned(),
                text: required(&pairs, "tran_id", id).to_owned(),
            });
        }
    }
    (lines, details)
}

/// The value of the first `key=` pair of `pairs`.
fn pair<'p>(pairs: &'p [KeyValue], key: &str) -> Option<&'p str> {
    pairs
        .iter()
        .find(|pair| pair.key == key)
        .map(|pair| pair.value.as_str())
}

/// The value of the first `key=` pair of `pairs`, which row `id` must hold.
fn required<'p>(pairs: &'p [KeyValue], key: &str, id: &str) -> &'p str {
    pair(pairs, key).unwrap_or_else(|| panic!("parity audit row {id} has no {key}="))
}

/// The `key=` count of the `Expected result` cell of row `id`.
fn count(doc: &Document, id: &str, key: &str) -> usize {
    let value = expected_value(doc, id, key);
    value.parse().unwrap_or_else(|error| {
        panic!("parity audit row {id}: {key}={value} is not a count: {error}")
    })
}

/// The value of the exact text `span` of row `id`: its double-quoted string right-padded with
/// spaces to its `width=`, which must be `width`.
///
/// # Panics
///
/// Panics if the span is not a double-quoted string, has another width or none, is longer
/// than its width, or stands for bytes the COBOL never sets (`{UNDEF:n}`, E-25), which a
/// byte-for-byte report line cannot hold.
fn exact_text(span: &CodeSpan, width: usize, id: &str) -> String {
    assert_eq!(span.width, Some(width), "{id}: text= width");
    let text = unquote(&span.text, id);
    assert!(
        !text.contains("{UNDEF:"),
        "{id}: text= holds bytes the COBOL never sets (E-25)"
    );
    assert!(
        text.len() <= width,
        "{id}: text= is {} bytes, more than its width={width}",
        text.len()
    );
    format!("{text:<width$}")
}

/// The value of the double-quoted exact text `content` of row `id`, whose only escapes are
/// `\"` and `\\` (the audit's conventions).
fn unquote(content: &str, id: &str) -> String {
    let inner = content
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_else(|| panic!("{id}: {content:?} is not a double-quoted exact text"));
    let mut text = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some(escaped @ ('"' | '\\')) => text.push(escaped),
                _ => {
                    panic!("{id}: {content:?} holds a backslash that escapes no quote or backslash")
                }
            },
            '"' => panic!("{id}: {content:?} holds an unescaped quote"),
            _ => text.push(ch),
        }
    }
    text
}

/// `TRANSACT.BKUP(+1)` against the live master and audit D (P-TRANREPT-001, P-CBTRN02C-216,
/// P-CBTRN02C-220): one record per posted transaction, in strictly ascending TRAN-ID order,
/// each the live master row encoded through the codec, holding exactly the TRAN-IDs of the
/// D.6 detail lines, from the first to the last posted one, and stamped with the pinned
/// processing timestamp.
fn check_backup(audit: &Expected, master: &[Vec<Option<String>>], bkup: &[String]) {
    assert_eq!(
        bkup.len(),
        audit.posted,
        "TRANSACT.BKUP(+1) records against {POSTED} posted="
    );
    // The 20 FILLER bytes are left out: POSTTRAN never sets them, so their COBOL content is
    // unpredictable and the audit excludes them from comparison until the user confirms the
    // space fill (E-25, P-CVTRA05Y-018). Bytes 1 to 330 hold every field of the record.
    let expected: Vec<Line> = master.iter().map(|row| master_record(row)).collect();
    let content: Vec<String> = bkup
        .iter()
        .enumerate()
        .map(|(index, record)| field(record, RECORD_CONTENT, index + 1).to_owned())
        .collect();
    assert_same_lines(
        "TRANSACT.BKUP(+1) bytes 1-330 against the live master",
        &expected,
        &content,
    );

    let ids: Vec<&str> = bkup
        .iter()
        .enumerate()
        .map(|(index, record)| field(record, TRAN_ID, index + 1))
        .collect();
    for (index, pair) in ids.windows(2).enumerate() {
        if let [before, after] = pair {
            assert!(
                before < after,
                "TRANSACT.BKUP(+1) is not in strictly ascending TRAN-ID order, the KSDS key \
                 order: record {} holds {before}, record {} holds {after}",
                index + 1,
                index + 2
            );
        }
    }
    assert_eq!(
        (ids.first().copied(), ids.last().copied()),
        (Some(audit.first.as_str()), Some(audit.last.as_str())),
        "TRANSACT.BKUP(+1) first and last TRAN-ID against {POSTED} first= and last="
    );
    let held: BTreeSet<&str> = ids.iter().copied().collect();
    let reported: BTreeSet<&str> = audit
        .details
        .iter()
        .map(|line| line.text.as_str())
        .collect();
    let missing: Vec<&&str> = reported.difference(&held).collect();
    let extra: Vec<&&str> = held.difference(&reported).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "TRANSACT.BKUP(+1) TRAN-IDs against the {D6_SUBSECTION} detail lines: missing \
         {missing:?}, not reported {extra:?}"
    );
    for (index, record) in bkup.iter().enumerate() {
        let number = index + 1;
        assert_eq!(
            field(record, TRAN_PROC_TS, number),
            audit.proc_ts,
            "TRANSACT.BKUP(+1) record {number} TRAN-PROC-TS against {PINNED_PROC_TS} pinned="
        );
    }
}

/// The record content, bytes 1 to 330, of the live master `row` (the
/// [`LIVE_MASTER_COLUMNS`] in their text forms) encoded through the codec as batch writes
/// it, labelled by its TRAN-ID.
///
/// # Panics
///
/// Panics, naming the column, if a value is missing or does not fit its field.
fn master_record(row: &[Option<String>]) -> Line {
    let tran_id = column(row, 1);
    let amount = column(row, 6);
    let tran_amt = Decimal::from_str(amount)
        .map_err(|error| error.to_string())
        .and_then(|value| Money::<9>::from_decimal(value).map_err(|error| error.to_string()))
        .unwrap_or_else(|error| panic!("TRAN-ID {tran_id}: tran_amt {amount:?}: {error}"));
    let transaction = NewTransaction {
        tran_id: TranId::move_from(tran_id),
        tran_type_cd: TranTypeCode::move_from(column(row, 2)),
        tran_cat_cd: parsed(row, 3),
        tran_source: Alnum::move_from(column(row, 4)),
        tran_desc: Alnum::move_from(column(row, 5)),
        tran_amt,
        tran_merchant_id: parsed(row, 7),
        tran_merchant_name: Alnum::move_from(column(row, 8)),
        tran_merchant_city: Alnum::move_from(column(row, 9)),
        tran_merchant_zip: Alnum::move_from(column(row, 10)),
        tran_card_num: CardNumber::move_from(column(row, 11)),
        tran_orig_ts: Alnum::move_from(column(row, 12)),
        tran_proc_ts: Alnum::move_from(column(row, 13)),
    };
    let bytes = TransactionLayout::encode(&TransactionRecord::new(transaction), Charset::Ascii);
    let record = String::from_utf8(bytes).unwrap_or_else(|error| {
        panic!("TRAN-ID {tran_id}: the encoded record is not UTF-8: {error}")
    });
    Line {
        label: format!("TRAN-ID {tran_id}"),
        text: field(&record, RECORD_CONTENT, 1).to_owned(),
    }
}

/// The text of column `index` of the live master `row`.
fn column(row: &[Option<String>], index: usize) -> &str {
    row.get(index)
        .and_then(Option::as_deref)
        .unwrap_or_else(|| {
            panic!(
                "live master row {row:?} has no {} value",
                LIVE_MASTER_COLUMNS[index]
            )
        })
}

/// Column `index` of the live master `row`, a `9(n)` code, read into its field type.
fn parsed<T>(row: &[Option<String>], index: usize) -> T
where
    T: FromStr,
    T::Err: Display,
{
    let text = column(row, index);
    text.parse().unwrap_or_else(|error| {
        panic!(
            "live master column {} value {text:?}: {error}",
            LIVE_MASTER_COLUMNS[index]
        )
    })
}

/// `TRANSACT.DALY(+1)` against the sort of P-TRANREPT-002 and the D.6 order: as many records
/// as `records_in_range=`, each with its `TRAN-PROC-DT` inside the range and each a whole
/// backup record, in ascending card-number order, in exactly the TRAN-ID order of the D.6
/// detail lines.
fn check_sorted_daily(audit: &Expected, bkup: &[String], daly: &[String]) {
    assert_eq!(
        daly.len(),
        audit.records_in_range,
        "TRANSACT.DALY(+1) records against {SUMMARY} records_in_range="
    );
    let backup: BTreeMap<&str, &str> = bkup
        .iter()
        .enumerate()
        .map(|(index, record)| (field(record, TRAN_ID, index + 1), record.as_str()))
        .collect();
    let mut previous_card: Option<&str> = None;
    for (index, record) in daly.iter().enumerate() {
        let number = index + 1;
        let proc_dt = field(record, TRAN_PROC_DT, number);
        assert!(
            audit.start.as_str() <= proc_dt && proc_dt <= audit.end.as_str(),
            "TRANSACT.DALY(+1) record {number}: TRAN-PROC-DT {proc_dt:?} lies outside {}..={}",
            audit.start,
            audit.end
        );
        let card = field(record, TRAN_CARD_NUM, number);
        assert!(
            card.bytes().all(|byte| byte.is_ascii_digit()),
            "TRANSACT.DALY(+1) record {number}: TRAN-CARD-NUM {card:?} is not 16 zoned digits"
        );
        if let Some(before) = previous_card {
            assert!(
                before <= card,
                "TRANSACT.DALY(+1) record {number}: card {card} follows card {before}"
            );
        }
        previous_card = Some(card);
        let tran_id = field(record, TRAN_ID, number);
        assert_eq!(
            backup.get(tran_id).copied(),
            Some(record.as_str()),
            "TRANSACT.DALY(+1) record {number} (TRAN-ID {tran_id}) is not a whole \
             TRANSACT.BKUP(+1) record"
        );
    }
    // Equal card numbers have no defined DFSORT order (E-38, a halt escalation). The expected
    // order is the disposition the reviewed D.6 lines follow, TRAN-ID within a card
    // (P-COMBTRAN-009); E-38 stays open for sign-off.
    let order: Vec<String> = daly
        .iter()
        .enumerate()
        .map(|(index, record)| field(record, TRAN_ID, index + 1).to_owned())
        .collect();
    assert_same_lines(
        "TRANSACT.DALY(+1) TRAN-ID order against the D.6 detail lines",
        &audit.details,
        &order,
    );
}

/// The bytes `range` of `record`, record `number` of its file.
fn field(record: &str, range: Range<usize>, number: usize) -> &str {
    let (start, end) = (range.start, range.end);
    record
        .get(range)
        .unwrap_or_else(|| panic!("record {number} has no bytes {} to {end}", start + 1))
}

/// Fails at the first line where `actual` departs from `expected`, naming its number and
/// label and showing both texts with their trailing spaces made visible, or, when one list
/// is a prefix of the other, at the first line only one of them holds.
fn assert_same_lines(what: &str, expected: &[Line], actual: &[String]) {
    if let Some(index) = expected
        .iter()
        .zip(actual)
        .position(|(line, text)| line.text != *text)
    {
        panic!(
            "{what}: line {} ({}) differs\nexpected: {}\nactual:   {}",
            index + 1,
            expected[index].label,
            visible(&expected[index].text),
            visible(&actual[index])
        );
    }
    let first_unmatched = match (expected.get(actual.len()), actual.get(expected.len())) {
        (Some(missing), _) => format!("missing {} {}", missing.label, visible(&missing.text)),
        (None, Some(extra)) => format!("unexpected {}", visible(extra)),
        (None, None) => return,
    };
    panic!(
        "{what}: {} lines written, {} expected; the first {} agree; line {}: {first_unmatched}",
        actual.len(),
        expected.len(),
        actual.len().min(expected.len()),
        actual.len().min(expected.len()) + 1
    );
}

/// `text` in brackets with each trailing space shown as `·`, and its length.
fn visible(text: &str) -> String {
    let kept = text.trim_end_matches(' ');
    let trailing = text.len() - kept.len();
    format!(
        "[{kept}{}] ({} bytes, {trailing} trailing spaces shown as ·)",
        "·".repeat(trailing),
        text.len()
    )
}

/// The date filter of the CBTRN03C read loop [app/cbl/CBTRN03C.cbl:L170-178]: `NEXT SENTENCE`
/// at the first record outside the range leaves the whole loop, with no further record and no
/// totals, while the `STEP05R` `INCLUDE` with the same dates keeps every such record out of the
/// input.
const DATE_FILTER: &str = "P-CBTRN03C-003";

/// Four transactions of two fixture cards around the default range, in TRAN-ID order:
/// processed the day before it starts, on its first day, on its last day and the day after it
/// ends. The columns are those of the view `transaction_master` of migration `0003`.
const AROUND_THE_RANGE: &str = r"
INSERT INTO transaction_master
    (tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt, tran_merchant_id,
     tran_merchant_name, tran_merchant_city, tran_merchant_zip, tran_card_num, tran_orig_ts,
     tran_proc_ts)
VALUES ('0000000000000001', '01', 1, 'POS TERM', 'Purchase at Example Store', 10.00, 800000001,
        'Example Store', 'Example City', '12345', '0500024453765740',
        '2021-12-31 19:27:53.000000', '2021-12-31-12.00.00.000000'),
       ('0000000000000002', '01', 1, 'POS TERM', 'Purchase at Example Store', 20.00, 800000001,
        'Example Store', 'Example City', '12345', '0500024453765740',
        '2022-01-01 19:27:53.000000', '2022-01-01-12.00.00.000000'),
       ('0000000000000003', '01', 1, 'POS TERM', 'Purchase at Example Store', 30.00, 800000001,
        'Example Store', 'Example City', '12345', '0683586198171516',
        '2022-07-06 19:27:53.000000', '2022-07-06-12.00.00.000000'),
       ('0000000000000004', '01', 1, 'POS TERM', 'Purchase at Example Store', 40.00, 800000001,
        'Example Store', 'Example City', '12345', '0683586198171516',
        '2022-07-07 19:27:53.000000', '2022-07-07-12.00.00.000000');
";

/// The fields CBTRN03C reports of one record of [`AROUND_THE_RANGE`], as its `INSERT` writes
/// them: `TRAN-ID`, `TRAN-TYPE-CD`, `TRAN-CAT-CD` as its `9(04)` digits, `TRAN-SOURCE`,
/// `TRAN-AMT` and `TRAN-CARD-NUM`.
struct Reported {
    tran_id: &'static str,
    type_cd: &'static str,
    cat_cd: &'static str,
    source: &'static str,
    amount: &'static str,
    card_num: &'static str,
}

/// Records 2 and 3 of [`AROUND_THE_RANGE`], processed on the first and on the last day of the
/// range, in the order of `TRANSACT.DALY(+1)`: one record on each card, the cards ascending.
const INSIDE: [Reported; 2] = [
    Reported {
        tran_id: "0000000000000002",
        type_cd: "01",
        cat_cd: "0001",
        source: "POS TERM",
        amount: "20.00",
        card_num: "0500024453765740",
    },
    Reported {
        tran_id: "0000000000000003",
        type_cd: "01",
        cat_cd: "0001",
        source: "POS TERM",
        amount: "30.00",
        card_num: "0683586198171516",
    },
];

#[test]
fn a_record_outside_the_dates_never_reaches_the_report_and_would_end_its_loop() {
    let doc = parity();
    let _ = parity_row(&doc, DATE_FILTER);
    let range = key_values(parity_cell(&doc, SUMMARY, "Input condition"));
    let (start, end) = (
        required(&range, "start", SUMMARY),
        required(&range, "end", SUMMARY),
    );
    let h = Harness::new();
    block_on(h.stack().exec_sql(AROUND_THE_RANGE));
    let _ = h.run_expect(&["tranrept"], Some(TRANREPT_CLOCK), SUCCESS);

    // The SORT INCLUDE uses the dates CBTRN03C filters by, so the program's input holds no
    // record outside them, and the report ends normally with its totals.
    let bkup = split_records(&h.newest("TRANSACT.BKUP"), TransactionLayout::LEN);
    let daly = split_records(&h.newest("TRANSACT.DALY"), TransactionLayout::LEN);
    let report = split_records(&h.newest("TRANREPT"), REPORT_LRECL);
    let all = tran_ids(&bkup);
    assert_eq!(
        all.len(),
        4,
        "{DATE_FILTER}: TRANSACT.BKUP(+1) holds every live record"
    );
    let in_range = [all[1], all[2]];
    assert_eq!(
        tran_ids(&daly),
        in_range,
        "{DATE_FILTER}: TRANSACT.DALY(+1)"
    );
    let details: Vec<&str> = report
        .iter()
        .filter_map(|line| line.get(TRAN_ID))
        .filter(|id| all.contains(id))
        .collect();
    assert_eq!(details, in_range, "{DATE_FILTER}: the detail lines");
    // The whole report against the audited CBTRN03C rules applied to the records inside the
    // range, never against the production report code.
    let expected = inside_report(&doc);
    assert_same_lines(
        &format!(
            "{DATE_FILTER}: TRANREPT(+1) against the audited report of the records inside the \
             range, totals included"
        ),
        &expected,
        &report,
    );

    // CBTRN03C itself, fed a record outside the range, writes nothing for it, nor for any record
    // after it, nor the end-of-input totals.
    let records: Vec<Transaction> = (1_i64..)
        .zip(&bkup)
        .map(|(number, bytes)| {
            let record = TransactionLayout::decode(bytes.as_bytes(), Charset::Ascii)
                .unwrap_or_else(|error| panic!("TRANSACT.BKUP(+1) record {number}: {error}"));
            Transaction::from_new(TransactionRowId::from(number), record.transaction)
        })
        .collect();
    let [before, first_day, last_day, after] = &records[..] else {
        panic!(
            "{DATE_FILTER}: TRANSACT.BKUP(+1) holds {} records, not 4",
            records.len()
        );
    };
    // The first record inside the range writes what it writes in the report: the header block
    // and its detail line (P-CBTRN03C-006).
    let first_detail: Vec<String> = expected[..=HEADER_BLOCK.len()]
        .iter()
        .map(|line| line.text.clone())
        .collect();
    let (left, left_totals) = report_run(&h, start, end, &[first_day, before, last_day]);
    assert_eq!(
        left,
        [first_detail, Vec::new(), Vec::new()],
        "{DATE_FILTER}: the record before the range leaves the loop"
    );
    assert_eq!(
        left_totals,
        Vec::<String>::new(),
        "{DATE_FILTER}: no totals"
    );
    let (first_out, first_out_totals) = report_run(&h, start, end, &[after, first_day, last_day]);
    assert_eq!(
        first_out,
        [Vec::<String>::new(), Vec::new(), Vec::new()],
        "{DATE_FILTER}: a first record after the range leaves the loop before any line"
    );
    assert_eq!(
        first_out_totals,
        Vec::<String>::new(),
        "{DATE_FILTER}: no totals"
    );
}

/// The TRAN-ID of each of `records`, in file order.
fn tran_ids(records: &[String]) -> Vec<&str> {
    records
        .iter()
        .enumerate()
        .map(|(index, record)| field(record, TRAN_ID, index + 1))
        .collect()
}

/// One run of CBTRN03C over the dates `start` to `end` against the lookup tables of the harness
/// database: the lines it writes for each of `records` in turn, then the lines it writes at the
/// end of its input.
fn report_run(
    h: &Harness,
    start: &str,
    end: &str,
    records: &[&Transaction],
) -> (Vec<Vec<String>>, Vec<String>) {
    let db = h.db();
    block_on(async {
        let mut uow = db
            .begin()
            .await
            .unwrap_or_else(|error| panic!("cannot begin the lookup unit of work: {error}"));
        let mut run = ReportRun::new(Alnum::move_from(start), Alnum::move_from(end));
        let mut written = Vec::new();
        {
            let mut store = uow.store();
            for tran in records {
                let lines = run
                    .process(&mut store, tran)
                    .await
                    .unwrap_or_else(|abend| panic!("a fixture lookup failed: {abend}"));
                written.push(lines);
            }
        }
        let totals = run.finish();
        uow.rollback()
            .await
            .unwrap_or_else(|error| panic!("cannot end the lookup unit of work: {error}"));
        (written, totals)
    })
}

/// The kinds of the four lines of the header block (P-CBTRN03C-475) in the order CBTRN03C
/// writes them, D.6 report lines 1 to 4 (P-CBTRN03C-016 to P-CBTRN03C-019).
const HEADER_BLOCK: [&str; 4] = [
    "REPORT-NAME-HEADER",
    "BLANK-LINE",
    "TRANSACTION-HEADER-1",
    "TRANSACTION-HEADER-2",
];

/// The card cross-reference fixture behind the `CARDXREF` lookup (P-CBTRN03C-001).
const XREF_FILE: &str = "cardxref.txt";

/// The `TRANTYPE` processing copy fixture (P-CBTRN03C-001).
const TRANTYPE_FILE: &str = "trantype.txt";

/// The `TRANCATG` processing copy fixture (P-CBTRN03C-001).
const TRANCATG_FILE: &str = "trancatg.txt";

/// The section A row of `XREF-CARD-NUM X(16)`, the `CARDXREF` key.
const XREF_CARD_NUM: &str = "P-CVACT03Y-001";

/// The section A row of `XREF-ACCT-ID 9(11)`, the account ID of a detail line.
const XREF_ACCT_ID: &str = "P-CVACT03Y-003";

/// The section A row of `TRAN-TYPE X(02)`, the `TRANTYPE` key.
const TRAN_TYPE: &str = "P-CVTRA03Y-001";

/// The section A row of `TRAN-TYPE-DESC X(50)`.
const TRAN_TYPE_DESC: &str = "P-CVTRA03Y-002";

/// The section A row of `TRAN-TYPE-CD X(02)`, the first part of the `TRANCATG` key.
const CAT_TYPE_CD: &str = "P-CVTRA04Y-001";

/// The section A row of `TRAN-CAT-CD 9(04)`, the second part of the `TRANCATG` key.
const CAT_CD: &str = "P-CVTRA04Y-002";

/// The section A row of `TRAN-CAT-TYPE-DESC X(50)`.
const CAT_TYPE_DESC: &str = "P-CVTRA04Y-003";

/// The section C row of the `+ZZZ,ZZZ,ZZZ.ZZ` edit of the account and grand totals, the picture
/// of the page total too (P-CBTRN03C-011).
const TOTALS_EDIT: &str = "P-CBTRN03C-012";

/// The section C row of the `-ZZZ,ZZZ,ZZZ.ZZ` edit of the detail amount.
const DETAIL_EDIT: &str = "P-CBTRN03C-014";

/// The width of both edited amount pictures.
const EDITED_WIDTH: usize = 15;

/// 10^11 cents: an `S9(09)V99` field holds 11 digits, so its magnitude stays below this.
const S9_09_V99_LIMIT: i64 = 100_000_000_000;

/// The sign symbol that begins an edited amount picture of CVTRA07Y.
#[derive(Clone, Copy)]
enum SignSymbol {
    /// `-ZZZ,ZZZ,ZZZ.ZZ`, `TRAN-REPORT-AMT` [app/cpy/CVTRA07Y.cpy:L30]: `-` when the amount is
    /// negative, otherwise a space.
    Minus,
    /// `+ZZZ,ZZZ,ZZZ.ZZ`, the page, account and grand totals [app/cpy/CVTRA07Y.cpy:L54]
    /// [app/cpy/CVTRA07Y.cpy:L60] [app/cpy/CVTRA07Y.cpy:L66]: `-` when the amount is negative,
    /// otherwise `+`.
    Plus,
}

impl SignSymbol {
    /// The picture, as section C writes it.
    fn picture(self) -> &'static str {
        match self {
            Self::Minus => "-ZZZ,ZZZ,ZZZ.ZZ",
            Self::Plus => "+ZZZ,ZZZ,ZZZ.ZZ",
        }
    }
}

/// A totals line of CVTRA07Y: a label `FILLER`, a `FILLER` of dots (`VALUE ALL '.'`) and the
/// total edited `+ZZZ,ZZZ,ZZZ.ZZ`.
struct TotalsLine {
    /// The record name, which also labels the line in a mismatch message.
    name: &'static str,
    /// The `VALUE` of the label `FILLER`.
    label: &'static str,
    /// The `PIC X(n)` length of the label `FILLER`.
    label_width: usize,
    /// The `PIC X(n)` length of the dot `FILLER`.
    dots: usize,
}

/// `REPORT-PAGE-TOTALS` [app/cpy/CVTRA07Y.cpy:L50-54].
const PAGE_TOTALS: TotalsLine = TotalsLine {
    name: "REPORT-PAGE-TOTALS",
    label: "Page Total",
    label_width: 11,
    dots: 86,
};

/// `REPORT-ACCOUNT-TOTALS` [app/cpy/CVTRA07Y.cpy:L56-60].
const ACCOUNT_TOTALS: TotalsLine = TotalsLine {
    name: "REPORT-ACCOUNT-TOTALS",
    label: "Account Total",
    label_width: 13,
    dots: 84,
};

/// `REPORT-GRAND-TOTALS` [app/cpy/CVTRA07Y.cpy:L62-66].
const GRAND_TOTALS: TotalsLine = TotalsLine {
    name: "REPORT-GRAND-TOTALS",
    label: "Grand Total",
    label_width: 11,
    dots: 86,
};

/// `TRANREPT(+1)` for the records [`INSIDE`] the default range, derived from the audited
/// CBTRN03C rules [app/cbl/CBTRN03C.cbl:L170-206] [app/cbl/CBTRN03C.cbl:L274-374] and the
/// fixtures alone, never from the production report code:
///
/// 1. The first detail writes the header block (P-CBTRN03C-006, P-CBTRN03C-475), which for the
///    default dates is D.6 lines 1 to 4. The line counter is then 4, and 7 before the second
///    detail, so `MOD(WS-LINE-COUNTER, 20)` is never 0 and no page break falls in this report.
/// 2. Each detail line (P-CBTRN03C-007) adds its amount to the page and account totals
///    (P-CBTRN03C-010).
/// 3. The second record changes the card, so before its detail come the first card's account
///    totals line and header 2, the account total reset to 0 (P-CBTRN03C-004, P-CBTRN03C-008,
///    P-CBTRN03C-012).
/// 4. At the end of input the last amount is added again (P-CBTRN03C-009, P-CBTRN03C-013), the
///    page totals line and header 2 are written, the page total is added to the grand total and
///    reset (P-CBTRN03C-011, P-CBTRN03C-486), and the grand totals line ends the report
///    (P-CBTRN03C-477). The second card's account total is never written.
///
/// Eleven lines: the header block, a detail, the account totals, header 2, a detail, the page
/// totals, header 2 and the grand totals.
fn inside_report(doc: &Document) -> Vec<Line> {
    assert_edits_match_audit(doc);
    let mut lines = header_block(&Expected::from_audit(doc));
    let header_2 = lines[HEADER_BLOCK.len() - 1].clone();
    let [first, second] = &INSIDE;
    assert!(
        first.card_num < second.card_num,
        "the records inside the range must lie on two cards in ascending order, the order of \
         TRANSACT.DALY(+1)"
    );
    let first_amount = cents(first.amount, first.tran_id);
    let second_amount = cents(second.amount, second.tran_id);
    let first_account_total = add(0, first_amount);
    let page_total = add(add(add(0, first_amount), second_amount), second_amount);
    let grand_total = add(0, page_total);
    lines.extend([
        detail_line(doc, first),
        total_line(&ACCOUNT_TOTALS, first_account_total),
        header_2.clone(),
        detail_line(doc, second),
        total_line(&PAGE_TOTALS, page_total),
        header_2,
        total_line(&GRAND_TOTALS, grand_total),
    ]);
    lines
}

/// D.6 lines 1 to 4, the header block CBTRN03C writes for the default dates before the first
/// detail (P-CBTRN03C-016 to P-CBTRN03C-019), each checked to be the line [`HEADER_BLOCK`]
/// names.
fn header_block(audit: &Expected) -> Vec<Line> {
    let lines = audit
        .lines
        .get(..HEADER_BLOCK.len())
        .unwrap_or_else(|| panic!("the {D6_SUBSECTION} lines hold no whole header block"));
    for (line, kind) in lines.iter().zip(HEADER_BLOCK) {
        assert!(
            line.label.ends_with(&format!(" {kind}")),
            "{D6_SUBSECTION}: {} is not the {kind} line of the header block",
            line.label
        );
    }
    lines.to_vec()
}

/// The `TRANSACTION-DETAIL-REPORT` line of `record` [app/cpy/CVTRA07Y.cpy:L15-31], in the order
/// of P-CBTRN03C-007, padded to the 133 bytes of `FD-REPTFILE-REC`: `TRAN-ID` X(16), a space,
/// the card's `XREF-ACCT-ID` X(11), a space, the type code X(02), `-`, the `TRANTYPE`
/// description X(15), a space, the category code 9(04), `-`, the `TRANCATG` description X(29),
/// a space, the source X(10), four spaces, the amount edited `-ZZZ,ZZZ,ZZZ.ZZ` (P-CBTRN03C-014)
/// and two spaces. A `MOVE` to a shorter alphanumeric field keeps the leading bytes.
fn detail_line(doc: &Document, record: &Reported) -> Line {
    let &Reported {
        tran_id,
        type_cd,
        cat_cd,
        source,
        amount,
        card_num,
    } = record;
    let account_id = lookup(doc, XREF_FILE, &[(XREF_CARD_NUM, card_num)], XREF_ACCT_ID);
    let type_desc = lookup(doc, TRANTYPE_FILE, &[(TRAN_TYPE, type_cd)], TRAN_TYPE_DESC);
    let cat_desc = lookup(
        doc,
        TRANCATG_FILE,
        &[(CAT_TYPE_CD, type_cd), (CAT_CD, cat_cd)],
        CAT_TYPE_DESC,
    );
    // The `9(11)` and `9(04)` sources move digit for digit into their `X(11)` and `9(04)`
    // receivers, which this line builds only from values of exactly those lengths.
    for (digits, width, what) in [
        (account_id.as_str(), 11, "XREF-ACCT-ID"),
        (cat_cd, 4, "TRAN-CAT-CD"),
    ] {
        assert!(
            digits.len() == width && digits.bytes().all(|byte| byte.is_ascii_digit()),
            "TRAN-ID {tran_id}: {what} {digits:?} is not {width} digits"
        );
    }
    let amount = edited(cents(amount, tran_id), SignSymbol::Minus);
    let text = format!(
        "{tran_id:<16.16} {account_id:<11.11} {type_cd:<2.2}-{type_desc:<15.15} \
         {cat_cd:<4.4}-{cat_desc:<29.29} {source:<10.10}    {amount}  "
    );
    Line {
        label: format!("TRANSACTION-DETAIL-REPORT of TRAN-ID {tran_id}"),
        text: format!("{text:<REPORT_LRECL$}"),
    }
}

/// The totals line `layout` holding `total` cents edited `+ZZZ,ZZZ,ZZZ.ZZ` (P-CBTRN03C-011,
/// P-CBTRN03C-012), padded to the 133 bytes of `FD-REPTFILE-REC`.
fn total_line(layout: &TotalsLine, total: i64) -> Line {
    let width = layout.label_width;
    let text = format!(
        "{:<width$}{}{}",
        layout.label,
        ".".repeat(layout.dots),
        edited(total, SignSymbol::Plus)
    );
    Line {
        label: layout.name.to_owned(),
        text: format!("{text:<REPORT_LRECL$}"),
    }
}

/// `cents` edited by the picture of `sign`: the sign symbol, the 9 integer digits in groups of
/// three, `.` and the 2 decimals. Zero suppression turns each leading zero and each comma before
/// the first significant digit into a space and stops at the point, so the decimals always show.
/// Every digit position is `Z`, so a zero amount is 15 spaces (P-CBTRN03C-012).
///
/// # Panics
///
/// Panics if `cents` does not fit `S9(09)V99`.
fn edited(cents: i64, sign: SignSymbol) -> String {
    assert!(
        (1 - S9_09_V99_LIMIT..S9_09_V99_LIMIT).contains(&cents),
        "{cents} cents do not fit S9(09)V99"
    );
    if cents == 0 {
        return " ".repeat(EDITED_WIDTH);
    }
    let symbol = match (cents < 0, sign) {
        (true, _) => '-',
        (false, SignSymbol::Minus) => ' ',
        (false, SignSymbol::Plus) => '+',
    };
    let magnitude = cents.unsigned_abs();
    let mut integer = String::new();
    let mut significant = false;
    for (index, digit) in format!("{:09}", magnitude / 100).chars().enumerate() {
        if index > 0 && index % 3 == 0 {
            integer.push(if significant { ',' } else { ' ' });
        }
        significant |= digit != '0';
        integer.push(if significant { digit } else { ' ' });
    }
    format!("{symbol}{integer}.{:02}", magnitude % 100)
}

/// `ADD amount TO total` into an `S9(09)V99` receiver without `ON SIZE ERROR`: the sum keeps
/// its sign and its 11 low-order digits (P-CBTRN03C-010, P-CBTRN03C-011: `mode=truncate`,
/// `high_order=truncate`).
fn add(total: i64, amount: i64) -> i64 {
    (total + amount) % S9_09_V99_LIMIT
}

/// The cents of the amount `text` that `source` gives: an optional `-`, one or more digits,
/// `.` and two digits, for example `-47.88`.
fn cents(text: &str, source: &str) -> i64 {
    let (negative, unsigned) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (whole, fraction) = unsigned
        .split_once('.')
        .filter(|(whole, fraction)| {
            !whole.is_empty()
                && fraction.len() == 2
                && whole
                    .bytes()
                    .chain(fraction.bytes())
                    .all(|byte| byte.is_ascii_digit())
        })
        .unwrap_or_else(|| panic!("{source}: {text:?} is not an amount with two decimals"));
    let value: i64 = format!("{whole}{fraction}")
        .parse()
        .unwrap_or_else(|error| panic!("{source}: amount {text:?}: {error}"));
    if negative { -value } else { value }
}

/// Fails unless [`edited`] gives every example of the `format=` of P-CBTRN03C-012
/// (`+ZZZ,ZZZ,ZZZ.ZZ`) and of P-CBTRN03C-014 (`-ZZZ,ZZZ,ZZZ.ZZ`), written
/// `<picture>: <amount> -> <edited text> width=15, <amount> -> ...`.
fn assert_edits_match_audit(doc: &Document) {
    for (id, sign) in [
        (TOTALS_EDIT, SignSymbol::Plus),
        (DETAIL_EDIT, SignSymbol::Minus),
    ] {
        let cell = expected_value(doc, id, "format");
        let (picture, examples) = cell
            .split_once(": ")
            .unwrap_or_else(|| panic!("{id}: format= names no picture"));
        assert_eq!(picture, sign.picture(), "{id}: format= picture");
        // Each `->` follows an amount, the last word before it, and precedes its edited text.
        let pieces: Vec<&str> = examples.split(" -> ").collect();
        let edits = cell_code_spans(examples);
        assert!(
            !edits.is_empty() && pieces.len() == edits.len() + 1,
            "{id}: format= holds no list of `<amount> -> <edited text>` examples"
        );
        for (&piece, edit) in pieces.iter().zip(&edits) {
            let amount = piece.rsplit(' ').next().unwrap_or(piece);
            assert_eq!(
                edit.width,
                Some(EDITED_WIDTH),
                "{id}: the width= of the edit of {amount}"
            );
            assert_eq!(
                edited(cents(amount, id), sign),
                unquote(&edit.text, id),
                "{id}: {amount} edited {picture}"
            );
        }
    }
}

/// Field `value` of the one record of the ASCII fixture `file` whose fields `keys` hold their
/// texts, every field a section A row: the keyed read of a CBTRN03C lookup (P-CBTRN03C-004)
/// from the fixture the seed loads (P-CBTRN03C-001).
///
/// # Panics
///
/// Panics, naming the file and the keys, unless exactly one record matches, or if a field lies
/// outside its record or is not UTF-8.
fn lookup(doc: &Document, file: &str, keys: &[(&str, &str)], value: &str) -> String {
    let text_at = |record: &[u8], range: &Range<usize>, number: usize| -> String {
        record
            .get(range.clone())
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .map_or_else(
                || {
                    panic!(
                        "{file} record {number}: no text at bytes {} to {}",
                        range.start + 1,
                        range.end
                    )
                },
                str::to_owned,
            )
    };
    let key_fields: Vec<(Range<usize>, &str)> = keys
        .iter()
        .map(|&(id, text)| (layout_range(doc, id), text))
        .collect();
    let value_field = layout_range(doc, value);
    let found: Vec<String> = fixture_lines(file)
        .iter()
        .zip(1_usize..)
        .filter(|&(record, number)| {
            key_fields
                .iter()
                .all(|(range, text)| text_at(record, range, number) == *text)
        })
        .map(|(record, number)| text_at(record, &value_field, number))
        .collect();
    match &found[..] {
        [one] => one.clone(),
        _ => panic!("{file}: {} records hold {keys:?}, not 1", found.len()),
    }
}

/// The bytes section A row `id` gives its field: `Offset` to `Offset + Length`.
fn layout_range(doc: &Document, id: &str) -> Range<usize> {
    let [offset, length] = [PARITY_LAYOUT_COLUMNS[0], PARITY_LAYOUT_COLUMNS[1]].map(|column| {
        let text = parity_cell(doc, id, column);
        text.parse::<usize>().unwrap_or_else(|error| {
            panic!("parity audit row {id}: {column} {text:?} is not a count: {error}")
        })
    });
    offset..offset + length
}
