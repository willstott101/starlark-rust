use pyo3::{
    exceptions::{PyRuntimeError, PySyntaxError},
    prelude::*,
    types::PyList,
};
use starlark::{
    environment::{Globals, Module},
    eval::Evaluator,
    syntax::{AstModule, Dialect},
    values::Value,
};
use starlark::values::list::List;
// use starlark::values::ValueLike;

fn starlark_type_to_pyo3_type(py: Python, v: &Value) -> Option<PyObject> {
    match v.get_type() {
        "string" => Some(v.to_str().to_object(py)),
        // array
        "bool" => Some(v.to_bool().to_object(py)),
        // dict
        // enum (int, string)?
        "float" => {
            if let Some(vf) = v.unpack_num().map(|n| n.as_float()) {
                Some(vf.to_object(py))
            } else {
                None
            }
        }
        // function
        "int" => {
            if let Some(vi) = v.unpack_num().map(|n| n.as_int()) {
                Some(vi.to_object(py))
            } else {
                None
            }
        }
        "list" => {
            match List::from_value(*v) {
                Some(l) => {
                    Some(PyList::new(py, l.iter().map(|i| starlark_type_to_pyo3_type(py, &i))).to_object(py))
                },
                None => None,
            }
        },
        "NoneType" => None,
        // range
        // record (FrozenDict?)
        // struct (FrozenDict?)
        // tuple
        _ => None,
    }
}

/// A Python module implemented in Rust. The name of this function must match
/// the `lib.name` setting in the `Cargo.toml`, else Python will not be able to
/// import the module.
#[pymodule]
fn starlark(_py: Python, m: &PyModule) -> PyResult<()> {
    #[pyfn(m)]
    fn eval(content: String) -> PyResult<Option<PyObject>> {
        // pyo3::prepare_freethreaded_python();
        let ast: AstModule = AstModule::parse("eval", content.to_owned(), &Dialect::Standard)
            .map_err(|err| PySyntaxError::new_err(err.to_string()))?;
        let globals: Globals = Globals::standard();
        let module: Module = Module::new();
        let mut eval: Evaluator = Evaluator::new(&module);

        // And finally we evaluate the code using the evaluator.
        let res: Value = eval
            .eval_module(ast, &globals)
            .map_err(|err| PyRuntimeError::new_err(err.to_string()))?;
        Python::with_gil(|py| -> PyResult<Option<PyObject>> {
            Ok(starlark_type_to_pyo3_type(py, &res))
        })
    }

    Ok(())
}
