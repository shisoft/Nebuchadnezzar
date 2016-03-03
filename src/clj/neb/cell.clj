(ns neb.cell
  (:require [neb.types :refer [data-types]]
            [neb.schema :refer [schema-store schema-by-id schema-id-by-sname walk-schema]]
            [cluster-connector.utils.for-debug :refer [spy $]])
  (:import (org.shisoft.neb trunk schemaStore)
           (org.shisoft.neb.io cellReader cellWriter reader type_lengths cellMeta)))

(def ^:dynamic ^cellMeta *cell-meta* nil)
(def ^:dynamic *cell-hash* nil)

(def cell-head-struc
  [[:hash :long :hash]
   [:schema-id :int :schema]])
(def cell-head-len
  (reduce + (map
              (fn [[_ type]]
                (get-in @data-types [type :length]))
              cell-head-struc)))

(defmacro with-cell [^cellReader cell-reader & body]
  `(let ~(vec (mapcat
                (fn [[n t]]
                  [(symbol (name n))
                   `(.streamRead
                      ~cell-reader
                      (get-in @data-types [~t :reader])
                      ~(get-in @data-types [ t :length]))])
                cell-head-struc))
     ~@body))

(defmacro gen-cell-header-offsets []
  (let [loc-counter (atom 0)]
    `(do ~@(map
             (fn [[prop type]]
               (let [{:keys [length]} (get @data-types type)
                     out-code `(def ~(symbol (str (name prop) "-offset")) ~(deref loc-counter))]
                 (swap! loc-counter (partial + length))
                 out-code))
             cell-head-struc)
         ~(do (reset! loc-counter 0) nil)
         (def cell-head-struc-map
           ~(into {}
                  (map
                    (fn [[prop type]]
                      (let [{:keys [length] :as fields} (get @data-types type)
                            res [prop (assoc (select-keys fields [:reader :writer]) :offset @loc-counter)]]
                        (swap! loc-counter (partial + length))
                        res))
                    cell-head-struc))))))

(defmacro with-cell-meta [trunk hash & body]
  `(with-bindings {#'*cell-meta* (-> ~trunk (.getCellIndex) (.get ~hash))
                   #'*cell-hash* ~hash}
     (when *cell-meta*
       ~@body)))

(defmacro with-write-lock [trunk hash & body]
  `(with-cell-meta
     ~trunk ~hash
     (locking *cell-meta*
       ~@body)))

(defmacro with-read-lock [trunk hash & body]
  `(with-cell-meta
     ~trunk ~hash
     (locking *cell-meta*
       ~@body)))

(defn get-cell-id []
  (.getLocation *cell-meta*))

(gen-cell-header-offsets)

(defn read-cell-header-field [^trunk trunk loc field]
  (let [{:keys [reader offset]} (get cell-head-struc-map field)]
    (reader trunk (+ loc offset))))

(defn write-cell-header-field [^trunk trunk loc field value]
  (let [{:keys [writer offset]} (get cell-head-struc-map field)]
    (writer trunk value (+ loc offset))))

(defn add-frag [^trunk trunk start end]
  (future     ;Use future to avoid deadlock with defragmentation daemon
    (locking (.getFragments trunk)
      (.addFragment trunk start end))))

(defn mark-cell-deleted [trunk cell-loc data-length]
  (add-frag trunk cell-loc (dec (+ cell-loc cell-head-len data-length))))

(defn calc-dynamic-type-length [trunk unit-length field-loc]
  (+ (* (reader/readInt trunk field-loc)
        unit-length)
     type_lengths/intLen))

;[[:id             :int]
; [:name           :text]
; [:map            [[:field1 :int] [:field2 :int]]]
; [:int-array     [:ARRRAY :int]]
; [:map-array      [:ARRAY [[:map-field :text]]]
; [:nested-array   [:ARRAY [:ARRAY :int]]]]

(def is-nested? vector?)
(def is-type? keyword?)

(defn walk-schema-for-read* [schema-fields ^cellReader cell-reader field-func map-func array-func]
  (walk-schema
    schema-fields
    map-func
    (fn [field-name field-format]
      (let [{:keys [unit-length length] :as type-props} (get @data-types field-format)
            field-length (or length (calc-dynamic-type-length trunk unit-length (.getCurrLoc cell-reader)))
            field-result (field-func field-name (.getCurrLoc cell-reader) type-props field-length)]
        (.advancePointer cell-reader field-length)
        field-result))
    (fn [field-name array-format]
      (let [array-len (reader/readInt trunk (.getCurrLoc cell-reader))
            recur-nested (fn [nested-schema & _] (walk-schema-for-read* nested-schema cell-reader field-func map-func array-func))]
        (.advancePointer cell-reader type_lengths/intLen)
        (apply
          array-func
          (doall
            (repeatedly
              array-len
              (fn []
                (cond
                  (is-nested? array-format)
                  (recur-nested array-format)
                  (is-type? array-format)
                  (if (get @data-types array-format)
                    (:d (recur-nested [[:d array-format]]))
                    (recur-nested (schema-id-by-sname array-format))))))))))))

(defn walk-schema-for-read [schema-fields ^Long cell-loc field-func map-func array-func]
  (walk-schema-for-read* schema-fields (cellReader. trunk cell-loc) field-func map-func array-func))

(defn walk-schema-for-write
  "It was assumed to have some side effect"
  [schema-fields data field-func map-func array-func array-header-func]
  (walk-schema
    schema-fields
    map-func
    (fn [field-name field-format]
      (field-func (get data field-name) field-name field-format))
    (fn [array-name array-format]
      (let [array-items (get data array-name)
            array-length (count array-field)
            array-header (array-header-func array-length)
            nested-format? (is-nested? array-format)
            type-format?   (is-type? array-format)
            recur-nested (fn [nested-schema data & _] (walk-schema-for-write nested-schema data field-func map-func array-func array-header-func))
            array-content
            (doall (map
                     (fn [item]
                       (cond
                         nested-format?
                         (recur-nested array-format item)
                         type-format?
                         (if (get @data-types array-format)
                           (:d (recur-nested [[:d array-format]]))
                           (recur-nested (schema-id-by-sname array-format)))))
                     array-items))]
        (apply array-func array-name array-format array-header array-content)))))

(defn calc-trunk-cell-length [^trunk trunk ^Long cell-loc schema]
  (let [cell-data-loc (+ cell-loc cell-head-len)
        cell-reader (cellReader. trunk cell-data-loc)]
    (reduce + (map
                (fn [[_ data-type]]
                  (if (vector? data-type)
                    (calc-trunk-cell-length trunk (.getCurrLoc cell-reader) data-type)
                    (let [{:keys [unit-length length]} (get @data-types data-type)
                          field-length (or length (calc-dynamic-type-length trunk unit-length (.getCurrLoc cell-reader)))]
                      (.advancePointer cell-reader field-length)
                      field-length)))
                (:f schema)))))

(defn delete-cell [^trunk trunk ^Long hash]
  (with-write-lock
    trunk hash
    (if-let [cell-loc (get-cell-id)]
      (let [schema-id (read-cell-header-field trunk cell-loc :schema-id)
            schema (schema-by-id schema-id)
            data-length (calc-trunk-cell-length trunk cell-loc schema)]
        (.removeCellFromIndex trunk hash)
        (mark-cell-deleted trunk cell-loc data-length))
      (throw (Exception. "Cell hash does not existed to delete")))))

(defn read-cell* [^trunk trunk]
  (if-let [loc (get-cell-id)]
    (let [cell-reader (cellReader. trunk loc)]
      (with-cell
        cell-reader
        (when-let [schema (schema-by-id schema-id)]
          (merge (into
                   {}
                   (map
                     (fn [[key-name data-type]]
                       [key-name
                        (let [{:keys [length reader dep dynamic? decoder unit-length]} (get @data-types data-type)
                              dep (when dep (get @data-types dep))
                              reader (or reader (get dep :reader))
                              reader (if decoder (comp decoder reader) reader)
                              length (or length (calc-dynamic-type-length trunk unit-length (.getCurrLoc cell-reader)))]
                          (.streamRead cell-reader reader length))])
                     (:f schema)))
                 {:*schema* schema-id
                  :*hash*   *cell-hash*}))))))

(defn read-cell [^trunk trunk ^Long hash]
  (with-read-lock
    trunk hash
    (read-cell* trunk)))

(defmacro write-cell-header [cell-writer header-data]
  `(do ~@(map
           (fn [[head-name head-type head-data-func]]
             (let [{:keys [length]} (get @data-types head-type)]
               `(.streamWrite
                  ~cell-writer
                  (get-in @data-types [~head-type :writer])
                  (~head-data-func ~header-data)
                  ~length)))
           cell-head-struc)))

(defn cell-fields-to-write [schema data]
  (map
    (fn [[key-name data-type]]
      (let [{:keys [length writer dep dynamic? encoder
                    unit-length count-array-length count-length]} (get @data-types data-type)
            dep (when dep (get @data-types dep))
            writer (or writer (get dep :writer))
            field-data (get data key-name)
            field-data (if encoder (encoder field-data) field-data)]
        {:key-name key-name
         :type data-type
         :value field-data
         :writer writer
         :length (if dynamic?
                   (cond
                     count-array-length
                     (+ (* (count-array-length field-data)
                           unit-length)
                        type_lengths/intLen)
                     count-length
                     (count-length field-data))
                   length)}))
    (:f schema)))

(defn cell-len-by-fields [fields-to-write]
  (reduce + (map :length fields-to-write)))

(defmacro locking-index [^trunk trunk & body]
  `(locking (.getCellIndex ~trunk)
     ~@body))

(defn write-cell [^trunk trunk ^Long hash schema data & {:keys [loc update-cell? update-hash-index?] :or {update-hash-index? true}}]
  (let [schema-id (:i schema)
        fields (cell-fields-to-write schema data)
        fields-length (cell-len-by-fields fields)
        cell-length (+ cell-head-len fields-length)
        cell-writer (if loc
                      (cellWriter. ^trunk trunk ^Long cell-length loc)
                      (cellWriter. ^trunk trunk ^Long cell-length))
        header-data {:schema schema-id
                     :hash hash}]
    (write-cell-header cell-writer header-data)
    (doseq [{:keys [key-name type value writer length] :as field} fields]
      (.streamWrite cell-writer writer value length))
    (when update-hash-index?
      (locking-index
        trunk
        (if update-cell?
          (.updateCellToTrunkIndex cell-writer hash)
          (.addCellToTrunkIndex cell-writer hash))))))

(defn new-cell [^trunk trunk ^Long hash ^Integer schema-id data]
  (when (.hasCell trunk hash)
    (throw (Exception. "Cell hash already exists")))
  (when-let [schema (schema-by-id schema-id)]
    (write-cell trunk hash schema data)))

(defn replace-cell* [^trunk trunk ^Long hash data]
  (when-let [cell-loc (get-cell-id)]
    (let [cell-data-loc (+ cell-loc cell-head-len)
          schema-id (read-cell-header-field trunk cell-loc :schema-id)
          schema (schema-by-id schema-id)
          data-len (calc-trunk-cell-length trunk cell-loc schema)
          fields (cell-fields-to-write schema data)
          new-data-length (cell-len-by-fields fields)]
      (if (>= data-len new-data-length)
        (do (write-cell trunk hash schema data :loc cell-loc :update-hash-index? false)
            (when (< new-data-length data-len)
              (add-frag
                trunk
                (+ cell-data-loc new-data-length 1)
                (+ cell-data-loc data-len))))
        (do (write-cell trunk hash schema data :update-cell? true)
            (mark-cell-deleted trunk cell-loc data-len))))))

(defn replace-cell [^trunk trunk ^Long hash data]
  (with-write-lock
    trunk hash
    (replace-cell* trunk hash data)))

(defn update-cell [^trunk trunk ^Long hash fn & params]  ;TODO Replace with less overhead function
  (with-write-lock
    trunk hash
    (when-let [cell-content (read-cell* trunk)]
      (let [replacement  (apply fn cell-content params)]
        (replace-cell* trunk hash replacement)
        replacement))))