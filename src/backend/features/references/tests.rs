use crate::backend::features::references::find_references;
use crate::backend::syntax::parser::extract_document_symbols;
use crate::backend::test_utils::parse_text;
use tower_lsp::lsp_types::{
    Position, ReferenceContext, ReferenceParams, TextDocumentIdentifier,
    TextDocumentPositionParams, Url,
};

fn setup_analysis(code: &str) -> (crate::analysis::Analysis, Url) {
    let uri = Url::parse("file:///test.vhd").unwrap();
    let tree = parse_text(code);
    let analysis = extract_document_symbols(code, tree.root_node());
    (analysis, uri)
}

fn make_params(uri: &Url, line: u32, character: u32) -> ReferenceParams {
    ReferenceParams {
        text_document_position: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position: Position { line, character },
        },
        context: ReferenceContext {
            include_declaration: true,
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    }
}

#[test]
fn test_references_local_signal() {
    let code = "
        architecture rtl of top is
        signal my_sig : std_logic;
    begin
        my_sig <= '1';
    process(my_sig)
        begin
        end process;
    end architecture;";
    let (analysis, uri) = setup_analysis(code);
    // Cursor on 'my_sig' in the declaration
    let params = make_params(&uri, 2, 10);
    let mut locs = find_references(&params, &analysis, &uri, "my_sig");

    locs.sort_by_key(|l| (l.range.start.line, l.range.start.character));
    let lines: Vec<u32> = locs.iter().map(|l| l.range.start.line).collect();
    // Expect declaration (line 2), assignment (line 4), and sensitivity list (line 5)
    assert_eq!(lines, vec![2, 4, 5]);
}

#[test]
fn test_references_process_variable() {
    let code = "
        architecture rtl of top is
        begin
        process
        variable my_var : integer := 0;
    begin
        my_var := my_var + 1;
    end process;
    end architecture;";
    let (analysis, uri) = setup_analysis(code);
    // Cursor on 'my_var' in the assignment
    let params = make_params(&uri, 6, 10);
    let mut locs = find_references(&params, &analysis, &uri, "my_var");

    locs.sort_by_key(|l| (l.range.start.line, l.range.start.character));
    let lines: Vec<u32> = locs.iter().map(|l| l.range.start.line).collect();
    // Expect declaration (line 4) and two usages on line 6 (LHS and RHS)
    assert_eq!(lines, vec![4, 6, 6]);
}

#[test]
fn test_references_entity_port() {
    let code = "
        entity top is
        port ( clk : in std_logic );
    end entity;
    architecture rtl of top is
        begin
        process(clk)
        begin
        end process;
    end architecture;";
    let (analysis, uri) = setup_analysis(code);
    // Cursor on 'clk' in the sensitivity list
    let params = make_params(&uri, 6, 10);
    let mut locs = find_references(&params, &analysis, &uri, "clk");

    locs.sort_by_key(|l| (l.range.start.line, l.range.start.character));
    let lines: Vec<u32> = locs.iter().map(|l| l.range.start.line).collect();
    // Expect declaration (line 2) and usage in sensitivity list (line 6)
    assert_eq!(lines, vec![2, 6]);
}

#[test]
fn test_references_undeclared_symbol() {
    let code = "
        architecture rtl of top is
        begin
        process
        begin
        foo <= '1';
    end process;
    end architecture;";
    let (analysis, uri) = setup_analysis(code);
    let params = make_params(&uri, 5, 5);
    let locs = find_references(&params, &analysis, &uri, "foo");
    // Should definitely be empty
    assert!(
        locs.is_empty(),
        "Expected no references for undeclared symbol 'foo', got {} locations",
        locs.len()
    );
}

#[test]
fn test_references_isolated_scopes() {
    let code = "
        architecture rtl of top is
        begin
        PROC1: process
        variable my_var : integer := 0;
    begin
        my_var := 1;
    end process;

    PROC2: process
        variable my_var : integer := 0;
    begin
        my_var := 2;
    end process;
    end architecture;";
    let (analysis, uri) = setup_analysis(code);

    // Cursor on 'my_var' in PROC1's assignment
    let params = make_params(&uri, 6, 10);
    let mut locs = find_references(&params, &analysis, &uri, "my_var");

    locs.sort_by_key(|l| (l.range.start.line, l.range.start.character));
    let lines: Vec<u32> = locs.iter().map(|l| l.range.start.line).collect();
    // We expect `locs` to only contain references from PROC1 (declaration at line 4, assignment at line 6)
    // and NOT from PROC2 (which are at lines 10 and 12).
    assert_eq!(
        lines,
        vec![4, 6],
        "References should not cross between isolated block scopes"
    );
}

#[test]
fn test_references_signal_attribute() {
    let code = "
architecture rtl of top is
  signal my_sig : std_logic_vector(7 downto 0);
begin
  process
    variable v_len : integer;
  begin
    v_len := my_sig'length;
  end process;
end architecture;";
    let (analysis, uri) = setup_analysis(code);

    // Cursor on 'my_sig' in the attribute usage
    let params = make_params(&uri, 7, 13);
    let mut locs = find_references(&params, &analysis, &uri, "my_sig");

    locs.sort_by_key(|l| (l.range.start.line, l.range.start.character));
    let lines: Vec<u32> = locs.iter().map(|l| l.range.start.line).collect();
    // Expect declaration (line 2) and usage in attribute (line 7)
    assert_eq!(lines, vec![2, 7]);
}
