//! SQL Server showplan XML (`SET SHOWPLAN_XML ON` for the estimated plan, `SET STATISTICS XML ON` for the
//! actual one) into a [`Plan`].
//!
//! No XML crate: the showplan is machine-written, well-formed XML, and all that is needed is elements,
//! attributes and nesting, which the small reader below provides. Text content is not used.

use std::collections::BTreeMap;
use theta_core::{Error, Plan, PlanNode, Result};

#[derive(Debug, Default)]
struct Element {
    name: String,
    attrs: BTreeMap<String, String>,
    children: Vec<Element>,
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Parse `xml` into a tree of elements. Comments, the prolog and text nodes are skipped.
fn parse_xml(xml: &str) -> Result<Element> {
    let b = xml.as_bytes();
    let mut i = 0;
    let mut stack: Vec<Element> = vec![Element::default()];
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        if xml[i..].starts_with("<!--") {
            i = xml[i..].find("-->").map_or(b.len(), |p| i + p + 3);
            continue;
        }
        if xml[i..].starts_with("<?") || xml[i..].starts_with("<!") {
            i = xml[i..].find('>').map_or(b.len(), |p| i + p + 1);
            continue;
        }
        // Find the end of the tag, skipping `>` inside quoted attribute values.
        let mut j = i + 1;
        let mut quote = 0u8;
        while j < b.len() {
            match (quote, b[j]) {
                (0, b'"' | b'\'') => quote = b[j],
                (q, c) if q != 0 && c == q => quote = 0,
                (0, b'>') => break,
                _ => {}
            }
            j += 1;
        }
        if j >= b.len() {
            return Err(Error::other("truncated showplan XML"));
        }
        let tag = &xml[i + 1..j];
        i = j + 1;
        if let Some(name) = tag.strip_prefix('/') {
            let done = stack
                .pop()
                .filter(|_| !stack.is_empty())
                .ok_or_else(|| Error::other("unbalanced showplan XML"))?;
            if done.name != name.trim() {
                return Err(Error::other(format!("mismatched </{}> in showplan XML", name.trim())));
            }
            stack.last_mut().unwrap().children.push(done);
            continue;
        }
        let self_closing = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        let name_end = tag.find(char::is_whitespace).unwrap_or(tag.len());
        let mut el = Element { name: tag[..name_end].to_string(), ..Default::default() };
        let mut rest = tag[name_end..].trim_start();
        while let Some(eq) = rest.find('=') {
            let key = rest[..eq].trim().to_string();
            let after = rest[eq + 1..].trim_start();
            let Some(q) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else { break };
            let Some(end) = after[1..].find(q) else { break };
            el.attrs.insert(key, unescape(&after[1..1 + end]));
            rest = after[end + 2..].trim_start();
        }
        if self_closing {
            stack.last_mut().unwrap().children.push(el);
        } else {
            stack.push(el);
        }
    }
    if stack.len() != 1 {
        return Err(Error::other("unclosed element in showplan XML"));
    }
    stack.pop().unwrap().children.into_iter().next().ok_or_else(|| Error::other("empty showplan XML"))
}

fn num(el: &Element, key: &str) -> Option<f64> {
    el.attrs.get(key).and_then(|v| v.parse().ok())
}

/// `[dbo]` / `[My Table]` to the bare name.
fn bare(name: &str) -> String {
    name.trim().trim_start_matches('[').trim_end_matches(']').to_string()
}

/// Every `RelOp` below `el` that is not below another `RelOp` (an operator's own inputs).
fn child_relops<'a>(el: &'a Element, out: &mut Vec<&'a Element>) {
    for c in &el.children {
        if c.name == "RelOp" {
            out.push(c);
        } else {
            child_relops(c, out);
        }
    }
}

fn first_descendant<'a>(el: &'a Element, name: &str) -> Option<&'a Element> {
    for c in &el.children {
        if c.name == name {
            return Some(c);
        }
        if c.name != "RelOp"
            && let Some(f) = first_descendant(c, name)
        {
            return Some(f);
        }
    }
    None
}

fn relop(el: &Element) -> PlanNode {
    let physical = el.attrs.get("PhysicalOp").cloned().unwrap_or_default();
    let logical = el.attrs.get("LogicalOp").cloned().unwrap_or_default();
    let mut n = PlanNode {
        node_type: physical.clone(),
        detail: Some(logical).filter(|l| !l.is_empty() && *l != physical),
        plan_rows: num(el, "EstimateRows"),
        total_cost: num(el, "EstimatedTotalSubtreeCost"),
        ..Default::default()
    };
    for (key, label) in [
        ("EstimateIO", "estimated I/O cost"),
        ("EstimateCPU", "estimated CPU cost"),
        ("AvgRowSize", "average row size"),
        ("Parallel", "parallel"),
        ("EstimatedExecutionMode", "execution mode"),
    ] {
        if let Some(v) = el.attrs.get(key) {
            n.extra.insert(label.into(), v.clone());
        }
    }
    // The object a scan or seek reads, and the predicate it applies.
    if let Some(obj) = first_descendant(el, "Object") {
        n.relation = obj.attrs.get("Table").map(|t| bare(t));
        n.alias = obj.attrs.get("Alias").map(|a| bare(a)).filter(|a| Some(a) != n.relation.as_ref());
        n.index = obj.attrs.get("Index").map(|i| bare(i));
        if let (Some(schema), Some(table)) = (obj.attrs.get("Schema"), &n.relation) {
            n.relation = Some(format!("{}.{table}", bare(schema)));
        }
    }
    if let Some(p) = first_descendant(el, "Predicate")
        && let Some(s) = first_descendant(p, "ScalarOperator").and_then(|s| s.attrs.get("ScalarString"))
    {
        n.filter = Some(s.clone());
    }
    // Measured values (actual plans): one `RunTimeCountersPerThread` per thread; rows add up, elapsed time
    // is the slowest thread's (threads run side by side).
    if let Some(info) = el.children.iter().find(|c| c.name == "RunTimeInformation") {
        let threads: Vec<&Element> =
            info.children.iter().filter(|c| c.name == "RunTimeCountersPerThread").collect();
        if !threads.is_empty() {
            n.actual_rows = Some(threads.iter().filter_map(|t| num(t, "ActualRows")).sum());
            n.loops = Some(threads.iter().filter_map(|t| num(t, "ActualExecutions")).sum());
            n.actual_total_ms = threads
                .iter()
                .filter_map(|t| num(t, "ActualElapsedms"))
                .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |x| x.max(v))));
            if let Some(read) = Some(threads.iter().filter_map(|t| num(t, "ActualRowsRead")).sum::<f64>())
                .filter(|r| *r > 0.0)
            {
                n.extra.insert("rows read".into(), format!("{read}"));
            }
        }
    }
    let mut kids = Vec::new();
    child_relops(el, &mut kids);
    n.children = kids.into_iter().map(relop).collect();
    n
}

/// A showplan document to a plan. `analyzed` says the document came from `STATISTICS XML` (it then carries
/// `RunTimeInformation`).
pub fn parse(xml: &str, analyzed: bool) -> Result<Plan> {
    let root = parse_xml(xml)?;
    fn find<'a>(el: &'a Element, name: &str) -> Option<&'a Element> {
        if el.name == name {
            return Some(el);
        }
        el.children.iter().find_map(|c| find(c, name))
    }
    let query_plan = find(&root, "QueryPlan").ok_or_else(|| Error::other("the showplan has no QueryPlan"))?;
    let top = query_plan
        .children
        .iter()
        .find(|c| c.name == "RelOp")
        .ok_or_else(|| Error::other("the showplan has no operators (the statement has no plan)"))?;
    let stmt_text = find(&root, "StmtSimple").and_then(|s| s.attrs.get("StatementText")).cloned();
    let mut root_node = relop(top);
    if let Some(t) = stmt_text {
        root_node.extra.insert("statement".into(), t);
    }
    // Compile time is the planning time; the actual plan also reports elapsed time for the whole statement.
    let planning_time_ms = num(query_plan, "CompileTime");
    let execution_time_ms =
        if analyzed { find(query_plan, "QueryTimeStats").and_then(|q| num(q, "ElapsedTime")) } else { None };
    Ok(Plan { root: root_node, analyzed, planning_time_ms, execution_time_ms, raw: xml.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Written from the showplan schema's element and attribute names (ShowPlanXML: StmtSimple, QueryPlan,
    // RelOp with PhysicalOp/LogicalOp/EstimateRows/EstimatedTotalSubtreeCost, IndexScan with Object, Predicate
    // with ScalarOperator/@ScalarString, RunTimeInformation/RunTimeCountersPerThread). The values are
    // invented; it is not output captured from a server, so it is weaker evidence than a capture.
    const ESTIMATED: &str = r#"<?xml version="1.0"?>
<ShowPlanXML xmlns="http://schemas.microsoft.com/sqlserver/2004/07/showplan" Version="1.6" Build="16.0.4135.4">
 <BatchSequence><Batch><Statements>
  <StmtSimple StatementText="SELECT e.name FROM emp e JOIN dept d ON d.id = e.dept_id WHERE e.name = 'ada'" StatementSubTreeCost="0.0066" StatementEstRows="1">
   <QueryPlan CachedPlanSize="24" CompileTime="3" CompileCPU="3" CompileMemory="208">
    <RelOp NodeId="0" PhysicalOp="Nested Loops" LogicalOp="Inner Join" EstimateRows="1" EstimateIO="0" EstimateCPU="4.18E-06" AvgRowSize="23" EstimatedTotalSubtreeCost="0.0066" Parallel="0" EstimatedExecutionMode="Row">
     <OutputList/>
     <NestedLoops Optimized="0">
      <RelOp NodeId="1" PhysicalOp="Clustered Index Scan" LogicalOp="Clustered Index Scan" EstimateRows="1" EstimateIO="0.003125" EstimateCPU="0.0001581" AvgRowSize="23" EstimatedTotalSubtreeCost="0.0032831" Parallel="0">
       <IndexScan Ordered="0" ForcedIndex="0" NoExpandHint="0">
        <Object Database="[app]" Schema="[dbo]" Table="[emp]" Index="[PK_emp]" Alias="[e]" IndexKind="Clustered" Storage="RowStore"/>
        <Predicate><ScalarOperator ScalarString="[app].[dbo].[emp].[name] as [e].[name]='ada'"><Compare/></ScalarOperator></Predicate>
       </IndexScan>
      </RelOp>
      <RelOp NodeId="2" PhysicalOp="Clustered Index Seek" LogicalOp="Clustered Index Seek" EstimateRows="1" EstimateIO="0.003125" EstimateCPU="0.0001581" AvgRowSize="9" EstimatedTotalSubtreeCost="0.0032831" Parallel="0">
       <IndexScan Ordered="1" ScanDirection="FORWARD" ForcedIndex="0" ForceSeek="0" NoExpandHint="0">
        <Object Database="[app]" Schema="[dbo]" Table="[dept]" Index="[PK_dept]" Alias="[d]" IndexKind="Clustered" Storage="RowStore"/>
       </IndexScan>
      </RelOp>
     </NestedLoops>
    </RelOp>
   </QueryPlan>
  </StmtSimple>
 </Statements></Batch></BatchSequence>
</ShowPlanXML>"#;

    #[test]
    fn estimated_plan_tree() {
        let p = parse(ESTIMATED, false).unwrap();
        assert_eq!(p.root.node_type, "Nested Loops");
        assert_eq!(p.root.detail.as_deref(), Some("Inner Join"));
        assert_eq!(p.root.plan_rows, Some(1.0));
        assert_eq!(p.root.total_cost, Some(0.0066));
        assert_eq!(p.planning_time_ms, Some(3.0));
        assert_eq!(p.root.children.len(), 2);
        let scan = &p.root.children[0];
        assert_eq!(scan.node_type, "Clustered Index Scan");
        assert_eq!(scan.relation.as_deref(), Some("dbo.emp"));
        assert_eq!(scan.index.as_deref(), Some("PK_emp"));
        assert_eq!(scan.alias.as_deref(), Some("e"));
        assert_eq!(scan.filter.as_deref(), Some("[app].[dbo].[emp].[name] as [e].[name]='ada'"));
        assert_eq!(p.root.children[1].node_type, "Clustered Index Seek");
        assert!(!p.analyzed);
    }

    #[test]
    fn actual_plan_runtime_counters() {
        let xml = r#"<ShowPlanXML><BatchSequence><Batch><Statements><StmtSimple>
          <QueryPlan CompileTime="1"><QueryTimeStats ElapsedTime="12" CpuTime="10"/>
           <RelOp PhysicalOp="Table Scan" LogicalOp="Table Scan" EstimateRows="10" EstimatedTotalSubtreeCost="0.01">
            <RunTimeInformation>
             <RunTimeCountersPerThread Thread="0" ActualRows="4" ActualRowsRead="10" ActualExecutions="1" ActualElapsedms="3"/>
             <RunTimeCountersPerThread Thread="1" ActualRows="3" ActualRowsRead="10" ActualExecutions="1" ActualElapsedms="5"/>
            </RunTimeInformation>
            <TableScan><Object Schema="[dbo]" Table="[t]"/></TableScan>
           </RelOp></QueryPlan></StmtSimple></Statements></Batch></BatchSequence></ShowPlanXML>"#;
        let p = parse(xml, true).unwrap();
        assert!(p.analyzed);
        assert_eq!(p.execution_time_ms, Some(12.0));
        assert_eq!(p.root.actual_rows, Some(7.0));
        assert_eq!(p.root.loops, Some(2.0));
        assert_eq!(p.root.actual_total_ms, Some(5.0));
        assert_eq!(p.root.extra.get("rows read").map(String::as_str), Some("20"));
        assert_eq!(p.root.relation.as_deref(), Some("dbo.t"));
    }

    #[test]
    fn malformed_and_empty_documents() {
        assert!(parse("<a><b></a>", false).is_err());
        assert!(parse("<a>", false).is_err());
        assert!(parse("<ShowPlanXML/>", false).is_err());
        // `>` inside an attribute value does not end the tag
        let p = parse(r#"<S><QueryPlan><RelOp PhysicalOp="Filter" EstimateRows="1"><X Y="a>b"/></RelOp></QueryPlan></S>"#, false).unwrap();
        assert_eq!(p.root.node_type, "Filter");
    }
}
